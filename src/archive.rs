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

/// Adjacent staging output that publishes atomically without replacing a destination.
pub(crate) struct AtomicOutput {
    destination: PathBuf,
    staging: PathBuf,
    file: Option<File>,
}
impl AtomicOutput {
    pub(crate) fn new(destination: impl AsRef<Path>) -> Result<Self> {
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
            match OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&staging)
            {
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
    pub(crate) fn file(&mut self) -> &mut File {
        self.file.as_mut().expect("staging writer closed")
    }
    pub(crate) fn staging_path(&self) -> &Path {
        &self.staging
    }
    pub(crate) fn finish(mut self) -> Result<()> {
        if let Some(f) = self.file.take() {
            f.sync_all()?;
            drop(f);
        }
        fs::hard_link(&self.staging, &self.destination)?;
        fs::remove_file(&self.staging)?;
        Ok(())
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
                backup.run_to_completion(128, Duration::from_millis(10), None)?;
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
