use crate::{
    error::{Error, Result},
    store::Store,
};
use rusqlite::{backup::Backup, Connection};
use std::{
    fs::{self, File, OpenOptions},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

static NEXT: AtomicU64 = AtomicU64::new(0);

/// Step SQLite's online backup until completion, but never wait indefinitely on a busy source.
pub fn backup_to_completion(
    backup: &Backup<'_, '_>,
    pages: i32,
    pause: Duration,
    timeout: Duration,
) -> Result<()> {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        match backup.step(pages)? {
            rusqlite::backup::StepResult::Done => return Ok(()),
            rusqlite::backup::StepResult::More => {}
            rusqlite::backup::StepResult::Busy | rusqlite::backup::StepResult::Locked | _ => {
                if std::time::Instant::now() >= deadline {
                    return Err(Error::Configuration(
                        "database backup busy timeout elapsed".into(),
                    ));
                }
            }
        }
        if std::time::Instant::now() >= deadline {
            return Err(Error::Configuration(
                "database backup deadline elapsed".into(),
            ));
        }
        std::thread::sleep(pause);
    }
}

/// Adjacent staging output that publishes atomically without replacing a destination.
pub struct AtomicOutput {
    destination: PathBuf,
    staging: PathBuf,
    file: Option<File>,
}
impl AtomicOutput {
    pub fn new(destination: impl AsRef<Path>) -> Result<Self> {
        let destination = destination.as_ref().to_path_buf();
        let parent = destination
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let name = destination
            .file_name()
            .ok_or_else(|| Error::Configuration("destination has no filename".into()))?
            .to_string_lossy();
        for _ in 0..128 {
            let n = NEXT.fetch_add(1, Ordering::Relaxed);
            let staging = parent.join(format!(".{name}.{}.{}.tmp", std::process::id(), n));
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            match options.open(&staging) {
                Ok(file) => {
                    return Ok(Self {
                        destination,
                        staging,
                        file: Some(file),
                    })
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e.into()),
            }
        }
        Err(Error::Configuration(
            "unable to reserve staging file".into(),
        ))
    }
    #[allow(dead_code)]
    pub fn file(&mut self) -> &mut File {
        self.file.as_mut().expect("staging writer closed")
    }
    pub fn staging_path(&self) -> &Path {
        &self.staging
    }
    pub fn finish(self) -> Result<()> {
        self.finish_with_parent_sync(sync_parent_directory)
    }

    fn finish_with_parent_sync(
        mut self,
        sync_parent: impl FnOnce(&Path) -> std::io::Result<()>,
    ) -> Result<()> {
        if let Some(f) = self.file.take() {
            f.sync_all()?;
            drop(f);
        }
        OpenOptions::new()
            .read(true)
            .write(true)
            .open(&self.staging)?
            .sync_all()?;
        let parent = self
            .destination
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        // Open before publication, so failure to open cannot make publication status ambiguous.
        let parent_file = File::open(parent)?;
        fs::hard_link(&self.staging, &self.destination)?;
        let unlink_result = fs::remove_file(&self.staging);
        #[cfg(unix)]
        let sync_result = sync_parent(parent);
        #[cfg(not(unix))]
        let sync_result = {
            let _ = (&sync_parent, &parent_file);
            Ok(())
        };
        if let Err(source) = unlink_result {
            return Err(Error::ArchivePublished {
                destination: self.destination.clone(),
                operation: "removing staging file",
                source,
            });
        }
        if let Err(source) = sync_result {
            return Err(Error::ArchivePublished {
                destination: self.destination.clone(),
                operation: "syncing parent directory",
                source,
            });
        }
        drop(parent_file);
        Ok(())
    }
}
fn sync_parent_directory(parent: &Path) -> std::io::Result<()> {
    File::open(parent)?.sync_all()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU64;

    static TEST_NEXT: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn sync_failure_reports_published_target_and_cleans_stage() {
        let dir = loop {
            let dir = std::env::temp_dir().join(format!(
                "archive-publish-{}-{}",
                std::process::id(),
                TEST_NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            match fs::create_dir(&dir) {
                Ok(()) => break dir,
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => panic!("reserve archive test directory: {error}"),
            }
        };
        let dest = dir.join("archive.db");
        let mut output = AtomicOutput::new(&dest).unwrap();
        use std::io::Write;
        output.file().write_all(b"complete archive").unwrap();
        let stage = output.staging_path().to_path_buf();
        let error = output
            .finish_with_parent_sync(|_| Err(std::io::Error::other("injected sync failure")))
            .unwrap_err();
        assert!(matches!(
            error,
            Error::ArchivePublished {
                operation: "syncing parent directory",
                ..
            }
        ));
        assert_eq!(fs::read(&dest).unwrap(), b"complete archive");
        assert!(!stage.exists());
        fs::remove_dir_all(dir).unwrap();
    }
}

impl Drop for AtomicOutput {
    fn drop(&mut self) {
        self.file.take();
        let _ = fs::remove_file(&self.staging);
        for ext in ["-wal", "-shm", "-journal"] {
            let _ = fs::remove_file(format!("{}{ext}", self.staging.display()));
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackupReceipt {
    pub destination: PathBuf,
    pub schema_version: i64,
    pub library_revision: i64,
    pub conversation_count: u64,
}

impl Store {
    pub fn backup_to(&self, destination: impl AsRef<Path>) -> Result<BackupReceipt> {
        let destination = destination.as_ref();
        let src = self.connection();
        if let (Ok(a), Ok(b)) = (
            fs::canonicalize(destination),
            fs::canonicalize(src.path().unwrap_or_default()),
        ) {
            if a == b {
                return Err(Error::Configuration(
                    "backup destination is source database".into(),
                ));
            }
        }
        let mut output = AtomicOutput::new(destination)?;
        drop(output.file.take());
        let result = (|| -> Result<BackupReceipt> {
            let mut target = Connection::open(output.staging_path())?;
            {
                let backup = Backup::new(src, &mut target)?;
                backup_to_completion(
                    &backup,
                    128,
                    Duration::from_millis(10),
                    Duration::from_secs(30),
                )?;
            }
            let schema_version =
                target.query_row("SELECT max(version) FROM schema_migrations", [], |r| {
                    r.get(0)
                })?;
            let library_revision = target.query_row(
                "SELECT revision FROM library_state WHERE singleton=1",
                [],
                |r| r.get(0),
            )?;
            let conversation_count =
                target.query_row("SELECT count(*) FROM conversations", [], |r| {
                    r.get::<_, i64>(0)
                })? as u64;
            target.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
            drop(target);
            Ok(BackupReceipt {
                destination: destination.to_path_buf(),
                schema_version,
                library_revision,
                conversation_count,
            })
        })();
        let receipt = result?;
        output.finish()?;
        Ok(receipt)
    }
}
