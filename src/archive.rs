use crate::{
    error::{Error, Result},
    store::Store,
};
use rusqlite::{backup::Backup, Connection, OpenFlags};
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
            rusqlite::backup::StepResult::More => std::thread::yield_now(),
            rusqlite::backup::StepResult::Busy | rusqlite::backup::StepResult::Locked | _ => {
                if std::time::Instant::now() >= deadline {
                    return Err(Error::Configuration(
                        "database backup busy timeout elapsed".into(),
                    ));
                }
                std::thread::sleep(pause);
            }
        }
        if std::time::Instant::now() >= deadline {
            return Err(Error::Configuration(
                "database backup deadline elapsed".into(),
            ));
        }
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestoreReceipt {
    pub schema_version: i64,
    pub library_revision: i64,
    pub conversation_count: u64,
}

/// A verified backup staged beside its destination so a desktop restore can
/// validate the file before stopping the active library service.
pub struct PreparedRestore {
    destination: PathBuf,
    staging: PathBuf,
    receipt: RestoreReceipt,
}

impl PreparedRestore {
    pub fn receipt(&self) -> &RestoreReceipt {
        &self.receipt
    }

    /// Replace the destination after its SQLite owner has shut down. The old
    /// database is kept until the staged database is in place; a failed rename
    /// attempts to restore the previous file before returning the error. Failed
    /// recovery preserves the remaining files and reports their locations.
    pub fn commit(self) -> Result<RestoreReceipt> {
        let parent = self
            .destination
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let previous = reserve_restore_path(parent, &self.destination, "previous")?;
        let had_previous = self.destination.exists();
        if had_previous {
            fs::remove_file(&previous)?;
            fs::rename(&self.destination, &previous)?;
        } else {
            fs::remove_file(&previous)?;
        }
        let sidecars = [
            PathBuf::from(format!("{}-wal", self.destination.display())),
            PathBuf::from(format!("{}-shm", self.destination.display())),
        ];
        let previous_sidecars = [
            PathBuf::from(format!("{}-wal", previous.display())),
            PathBuf::from(format!("{}-shm", previous.display())),
        ];
        for (source, target) in sidecars.iter().zip(previous_sidecars.iter()) {
            if source.exists() {
                if let Err(error) = fs::rename(source, target) {
                    return Err(rollback_restore(
                        error,
                        &self.destination,
                        &previous,
                        &sidecars,
                        &previous_sidecars,
                        had_previous,
                        None,
                    ));
                }
            }
        }
        if let Err(error) = fs::rename(&self.staging, &self.destination) {
            return Err(rollback_restore(
                error,
                &self.destination,
                &previous,
                &sidecars,
                &previous_sidecars,
                had_previous,
                None,
            ));
        }
        if let Err(error) = sync_parent_directory(parent) {
            return Err(rollback_restore(
                error,
                &self.destination,
                &previous,
                &sidecars,
                &previous_sidecars,
                had_previous,
                Some(&self.staging),
            ));
        }
        if had_previous {
            let _ = fs::remove_file(previous);
        }
        for sidecar in previous_sidecars {
            if sidecar.exists() {
                let _ = fs::remove_file(sidecar);
            }
        }
        let _ = sync_parent_directory(parent);
        Ok(self.receipt.clone())
    }
}

fn rollback_restore(
    original: std::io::Error,
    destination: &Path,
    previous: &Path,
    sidecars: &[PathBuf; 2],
    previous_sidecars: &[PathBuf; 2],
    had_previous: bool,
    published_staging: Option<&Path>,
) -> Error {
    let mut failures = Vec::new();
    if let Some(staging) = published_staging {
        if let Err(error) = fs::rename(destination, staging) {
            // Do not overwrite the installed database or attach old WAL files
            // when the newly installed database cannot be moved out of the way.
            return Error::RestoreRecovery { details: format!(
                "{original}; could not move the replacement aside: {error}. Library files are at {} and {}. Close Chat Hub and recover these files before restarting.",
                destination.display(), previous.display()
            ) };
        }
    }
    for (source, target) in previous_sidecars.iter().zip(sidecars.iter()) {
        if source.exists() {
            if let Err(error) = fs::rename(source, target) {
                failures.push(format!(
                    "{} -> {}: {error}",
                    source.display(),
                    target.display()
                ));
            }
        }
    }
    if had_previous {
        if let Err(error) = fs::rename(previous, destination) {
            failures.push(format!(
                "{} -> {}: {error}",
                previous.display(),
                destination.display()
            ));
        }
    }
    if failures.is_empty() {
        if let Some(parent) = destination.parent() {
            if let Err(error) = sync_parent_directory(parent) {
                failures.push(format!(
                    "could not confirm recovered library durability: {error}"
                ));
            }
        }
    }
    if failures.is_empty() {
        original.into()
    } else {
        Error::RestoreRecovery { details: format!(
            "{original}; rollback failed: {}. Close Chat Hub and recover the library and any WAL files at {} and {} before restarting.",
            failures.join("; "), previous.display(), destination.display()
        ) }
    }
}

impl Drop for PreparedRestore {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.staging);
        let _ = fs::remove_file(format!("{}-wal", self.staging.display()));
        let _ = fs::remove_file(format!("{}-shm", self.staging.display()));
    }
}

#[cfg(test)]
mod restore_recovery_tests {
    use super::*;

    #[test]
    fn rollback_failure_preserves_previous_database_and_reports_recovery_location() {
        let root = std::env::temp_dir().join(format!(
            "chat-hub-recovery-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();
        let destination = root.join("library.db");
        let previous = root.join("previous.db");
        fs::write(&previous, b"original database").unwrap();
        // A directory at the destination forces rollback's database rename to fail.
        fs::create_dir(&destination).unwrap();
        let sidecars = [root.join("library.db-wal"), root.join("library.db-shm")];
        let old_sidecars = [root.join("previous.db-wal"), root.join("previous.db-shm")];
        let error = rollback_restore(
            std::io::Error::other("swap failed"),
            &destination,
            &previous,
            &sidecars,
            &old_sidecars,
            true,
            None,
        );
        assert!(matches!(&error, Error::RestoreRecovery { .. }));
        assert!(error.to_string().contains(previous.to_str().unwrap()));
        assert_eq!(fs::read(&previous).unwrap(), b"original database");
        fs::remove_dir(&destination).unwrap();
        fs::write(&old_sidecars[0], b"original wal").unwrap();
        let error = rollback_restore(
            std::io::Error::other("swap failed"),
            &destination,
            &previous,
            &sidecars,
            &old_sidecars,
            true,
            None,
        );
        assert!(matches!(error, Error::Io(_)));
        assert_eq!(fs::read(&destination).unwrap(), b"original database");
        assert_eq!(fs::read(&sidecars[0]).unwrap(), b"original wal");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn failed_retraction_does_not_attach_old_wal_to_replacement() {
        let root = std::env::temp_dir().join(format!(
            "chat-hub-retraction-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();
        let destination = root.join("library.db");
        let previous = root.join("previous.db");
        let staging = root.join("staging");
        fs::write(&destination, b"replacement").unwrap();
        fs::write(&previous, b"original").unwrap();
        fs::create_dir(&staging).unwrap();
        let sidecars = [root.join("library.db-wal"), root.join("library.db-shm")];
        let old_sidecars = [root.join("previous.db-wal"), root.join("previous.db-shm")];
        fs::write(&old_sidecars[0], b"old wal").unwrap();
        let error = rollback_restore(
            std::io::Error::other("sync failed"),
            &destination,
            &previous,
            &sidecars,
            &old_sidecars,
            true,
            Some(&staging),
        );
        assert!(matches!(error, Error::RestoreRecovery { .. }));
        assert_eq!(fs::read(&destination).unwrap(), b"replacement");
        assert_eq!(fs::read(&previous).unwrap(), b"original");
        assert_eq!(fs::read(&old_sidecars[0]).unwrap(), b"old wal");
        assert!(!sidecars[0].exists());
        fs::remove_dir_all(root).unwrap();
    }
}

/// Validate and restore a backup into a private adjacent staging database.
/// Call `commit` only after the destination database has no live connections.
pub fn prepare_restore(
    source: impl AsRef<Path>,
    destination: impl AsRef<Path>,
) -> Result<PreparedRestore> {
    let source = source.as_ref();
    let destination = destination.as_ref().to_path_buf();
    if fs::canonicalize(source)? == fs::canonicalize(&destination)? {
        return Err(Error::Configuration(
            "restore source is destination database".into(),
        ));
    }
    let parent = destination
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let staging = reserve_restore_path(parent, &destination, "staging")?;
    let result = (|| {
        let mut store = Store::open(
            staging
                .to_str()
                .ok_or_else(|| Error::Configuration("restore path is not valid UTF-8".into()))?,
        )?;
        let receipt = store.restore_from(source)?;
        drop(store);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&staging, fs::Permissions::from_mode(0o600))?;
        }
        Ok::<_, Error>(receipt)
    })();
    match result {
        Ok(receipt) => Ok(PreparedRestore {
            destination,
            staging,
            receipt,
        }),
        Err(error) => {
            let _ = fs::remove_file(&staging);
            let _ = fs::remove_file(format!("{}-wal", staging.display()));
            let _ = fs::remove_file(format!("{}-shm", staging.display()));
            Err(error)
        }
    }
}

fn reserve_restore_path(parent: &Path, destination: &Path, purpose: &str) -> Result<PathBuf> {
    let name = destination
        .file_name()
        .ok_or_else(|| Error::Configuration("restore destination has no filename".into()))?
        .to_string_lossy();
    for _ in 0..128 {
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let candidate = parent.join(format!(
            ".{name}.{}.{}.restore-{purpose}",
            std::process::id(),
            n
        ));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match options.open(&candidate) {
            Ok(file) => {
                drop(file);
                return Ok(candidate);
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        }
    }
    Err(Error::Configuration(
        "unable to reserve restore staging file".into(),
    ))
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

    /// Restore a verified Chat Hub SQLite backup into an empty, file-backed library.
    /// The destination must be empty to prevent accidental replacement of user data.
    pub fn restore_from(&mut self, source: impl AsRef<Path>) -> Result<RestoreReceipt> {
        let source = source.as_ref();
        let destination = self
            .connection()
            .path()
            .filter(|path| !path.is_empty())
            .ok_or_else(|| {
                Error::Configuration("restore destination must be file-backed".into())
            })?;
        if fs::canonicalize(source)? == fs::canonicalize(destination)? {
            return Err(Error::Configuration(
                "restore source is destination database".into(),
            ));
        }
        let source_db = Connection::open_with_flags(
            source,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        let integrity: String = source_db.query_row("PRAGMA integrity_check", [], |r| r.get(0))?;
        if integrity != "ok" {
            return Err(Error::Configuration("backup integrity check failed".into()));
        }
        let foreign_key_errors: i64 =
            source_db.query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |r| {
                r.get(0)
            })?;
        if foreign_key_errors != 0 {
            return Err(Error::Configuration(
                "backup has foreign key violations".into(),
            ));
        }
        let schema_version: i64 =
            source_db.query_row("SELECT max(version) FROM schema_migrations", [], |r| {
                r.get(0)
            })?;
        let current_version = self.schema_version()?;
        if schema_version <= 0 || schema_version > current_version {
            return Err(Error::Configuration(
                "backup schema version is unsupported".into(),
            ));
        }
        let existing: i64 =
            self.connection()
                .query_row("SELECT count(*) FROM conversations", [], |r| r.get(0))?;
        if existing != 0 {
            return Err(Error::Configuration(
                "restore destination must be empty".into(),
            ));
        }
        let backup = Backup::new(&source_db, self.connection_mut())?;
        backup_to_completion(
            &backup,
            128,
            Duration::from_millis(10),
            Duration::from_secs(30),
        )?;
        drop(backup);
        self.migrate_for_restore()?;
        self.rebuild_search_index()?;
        let schema_version = self.schema_version()?;
        let library_revision = self.library_revision()?;
        let conversation_count =
            self.connection()
                .query_row("SELECT count(*) FROM conversations", [], |r| {
                    r.get::<_, i64>(0)
                })? as u64;
        self.connection()
            .execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
        Ok(RestoreReceipt {
            schema_version,
            library_revision,
            conversation_count,
        })
    }
}
