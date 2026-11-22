//! Private, bounded recovery for normalized live captures, before their DB commit.
//! This is not a packet journal: interrupted/unobserved streams remain unrecorded.
use crate::{
    error::{Error, Result},
    store::Store,
};
use chat_hub_model::{IngestOutcome, Observation};
use ring::digest::{digest, SHA256};
use serde::Serialize;
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

const ITEM_LIMIT: u64 = 8 * 1024 * 1024;
const BYTE_LIMIT: u64 = 64 * 1024 * 1024;
const ITEM_COUNT_LIMIT: usize = 512;
static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug, Default, Serialize)]
pub struct CaptureRecoveryStatus {
    pub pending_items: usize,
    pub retained_bytes: u64,
    pub incomplete_files: usize,
    pub recovered: usize,
    pub failed: usize,
}

fn error(code: &str) -> Error {
    Error::Configuration(code.to_owned())
}
fn sha256(data: &[u8]) -> String {
    digest(&SHA256, data)
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
fn sync_dir(path: &Path) -> Result<()> {
    #[cfg(unix)]
    File::open(path)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}
fn private_dir(path: &Path) -> Result<()> {
    if !path.exists() {
        let mut builder = fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        match builder.create(path) {
            Ok(()) => {
                if let Some(parent) = path.parent() {
                    sync_dir(parent)?;
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e.into()),
        }
    }
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(error("capture_journal_unsafe_directory"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

struct Journal {
    dir: PathBuf,
}
impl Journal {
    fn for_database(database: &str, create: bool) -> Result<Option<Self>> {
        if database.is_empty() || database == ":memory:" {
            return Ok(None);
        }
        let path = Path::new(database);
        let metadata = fs::metadata(path)?;
        // A restore replaces the database file. Its old pending transcripts must
        // not be replayed into the restored library. Retain them for manual review.
        #[cfg(unix)]
        let generation = {
            use std::os::unix::fs::MetadataExt;
            format!("{}:{}", metadata.dev(), metadata.ino())
        };
        #[cfg(not(unix))]
        let generation = format!("{:?}", metadata.created()?);
        let name = path
            .file_name()
            .ok_or_else(|| error("capture_journal_database_path"))?;
        let root = path.with_file_name(format!("{}.capture-journal", name.to_string_lossy()));
        let dir = root.join(sha256(generation.as_bytes()));
        if create {
            private_dir(&root)?;
            private_dir(&dir)?;
        } else if !dir.exists() {
            return Ok(None);
        }
        for candidate in [&root, &dir] {
            let meta = fs::symlink_metadata(candidate)?;
            if meta.file_type().is_symlink() || !meta.is_dir() {
                return Err(error("capture_journal_unsafe_directory"));
            }
        }
        Ok(Some(Self { dir }))
    }

    fn entries(&self) -> Result<(Vec<PathBuf>, CaptureRecoveryStatus)> {
        let mut paths = Vec::new();
        let mut status = CaptureRecoveryStatus::default();
        for (count, entry) in fs::read_dir(&self.dir)?.enumerate() {
            if count >= ITEM_COUNT_LIMIT * 2 {
                return Err(error("capture_journal_too_many_files"));
            }
            let entry = entry?;
            let metadata = fs::symlink_metadata(entry.path())?;
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                return Err(error("capture_journal_unsafe_entry"));
            }
            status.retained_bytes = status.retained_bytes.saturating_add(metadata.len());
            if entry.path().extension().is_some_and(|ext| ext == "json") {
                status.pending_items += 1;
                paths.push(entry.path());
            } else {
                status.incomplete_files += 1;
            }
        }
        paths.sort();
        Ok((paths, status))
    }

    fn read(&self, path: &Path) -> Result<Observation> {
        let metadata = fs::symlink_metadata(path)?;
        if metadata.file_type().is_symlink() || !metadata.is_file() || metadata.len() > ITEM_LIMIT {
            return Err(error("capture_journal_invalid_entry"));
        }
        let mut data = Vec::new();
        File::open(path)?
            .take(ITEM_LIMIT + 1)
            .read_to_end(&mut data)?;
        if data.len() as u64 > ITEM_LIMIT {
            return Err(error("capture_journal_oversized"));
        }
        let expected = format!("{}.json", sha256(&data));
        if path.file_name().and_then(|name| name.to_str()) != Some(expected.as_str()) {
            return Err(error("capture_journal_digest_mismatch"));
        }
        serde_json::from_slice(&data).map_err(|_| error("capture_journal_invalid_json"))
    }

    fn stage(&self, observation: &Observation) -> Result<PathBuf> {
        let data =
            serde_json::to_vec(observation).map_err(|_| error("capture_journal_encode_failed"))?;
        if data.len() as u64 > ITEM_LIMIT {
            return Err(error("capture_journal_oversized"));
        }
        let destination = self.dir.join(format!("{}.json", sha256(&data)));
        if destination.exists() {
            self.read(&destination)?;
            return Ok(destination);
        }
        let (_, status) = self.entries()?;
        if status.pending_items >= ITEM_COUNT_LIMIT
            || status.retained_bytes.saturating_add(data.len() as u64) > BYTE_LIMIT
        {
            return Err(error("capture_journal_full"));
        }
        let temp = self.dir.join(format!(
            ".pending-{}-{}",
            std::process::id(),
            NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
        ));
        let result = (|| {
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options.open(&temp)?;
            file.write_all(&data)?;
            file.sync_all()?;
            // Atomic no-replace publication, even with overlapping startup recovery.
            match fs::hard_link(&temp, &destination) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    self.read(&destination)?;
                }
                Err(e) => return Err(e.into()),
            }
            sync_dir(&self.dir)?;
            Ok(destination)
        })();
        let _ = fs::remove_file(&temp);
        result
    }

    fn acknowledge(&self, path: &Path) -> Result<()> {
        match fs::remove_file(path) {
            Ok(()) => sync_dir(&self.dir),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        }
    }
}

impl Store {
    pub fn capture_recovery_status(database: &str) -> Result<CaptureRecoveryStatus> {
        match Journal::for_database(database, false)? {
            Some(journal) => Ok(journal.entries()?.1),
            None => Ok(CaptureRecoveryStatus::default()),
        }
    }

    /// Replay this database generation only. Store ordering rules prevent an old
    /// capture from replacing a newer snapshot. Corrupt entries are kept, not erased.
    pub fn recover_pending_captures(&mut self) -> Result<CaptureRecoveryStatus> {
        let Some(database) = self.connection().path().map(str::to_owned) else {
            return Ok(CaptureRecoveryStatus::default());
        };
        let Some(journal) = Journal::for_database(&database, false)? else {
            return Ok(CaptureRecoveryStatus::default());
        };
        let (paths, mut status) = journal.entries()?;
        if status.retained_bytes > BYTE_LIMIT {
            return Err(error("capture_journal_full"));
        }
        for path in paths {
            let observation = match journal.read(&path) {
                Ok(value) => value,
                Err(_) => {
                    status.failed += 1;
                    continue;
                }
            };
            if self.ingest_observation_outcome(&observation).is_err() {
                status.failed += 1;
                // A busy/broken DB should not incur one timeout per queued entry.
                break;
            }
            if journal.acknowledge(&path).is_err() {
                status.failed += 1;
            } else {
                status.recovered += 1;
            }
        }
        let current = journal.entries()?.1;
        status.pending_items = current.pending_items;
        status.retained_bytes = current.retained_bytes;
        status.incomplete_files = current.incomplete_files;
        Ok(status)
    }

    pub(crate) fn store_captured_observation(
        &mut self,
        observation: &Observation,
    ) -> Result<IngestOutcome> {
        let Some(database) = self.connection().path().map(str::to_owned) else {
            return self.ingest_observation_outcome(observation);
        };
        let Some(journal) = Journal::for_database(&database, true)? else {
            return self.ingest_observation_outcome(observation);
        };
        // Retry failed captures independently of remote history and its HTTP 429s.
        let _ = self.recover_pending_captures();
        let ticket = journal.stage(observation)?;
        let result = self.ingest_observation_outcome(observation);
        if result.is_ok() {
            // A cleanup error cannot undo a confirmed DB commit. Keep the ticket;
            // replay is idempotent for the visible library and status exposes it.
            let _ = journal.acknowledge(&ticket);
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chat_hub_model::ObservationSink;
    use serde_json::json;

    struct Fixture {
        dir: PathBuf,
        database: String,
    }
    impl Fixture {
        fn new() -> Self {
            let mut random = [0u8; 16];
            use ring::rand::SecureRandom;
            ring::rand::SystemRandom::new().fill(&mut random).unwrap();
            let dir = std::env::temp_dir().join(format!("chat-hub-journal-{}", sha256(&random)));
            fs::create_dir(&dir).unwrap();
            let database = dir.join("library.sqlite3").to_str().unwrap().to_owned();
            Self { dir, database }
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.dir);
        }
    }
    fn observation(scope: &str) -> Observation {
        Observation {
            provider: "chatgpt".into(),
            account_scope: scope.into(),
            source_id: "synthetic-source".into(),
            title: "Synthetic capture".into(),
            url: None,
            current_node: None,
            graph: vec![],
            raw_payload: json!({"title":"Synthetic capture"}),
            source_time: None,
            observed_ms: 1,
            parser: "chatgpt-live-v1".into(),
            completeness: "complete".into(),
        }
    }
    #[test]
    fn failed_commit_is_retained_and_replayed_after_restart() {
        let f = Fixture::new();
        let mut store = Store::open_service(&f.database).unwrap();
        store
            .connection()
            .execute_batch("PRAGMA query_only=ON")
            .unwrap();
        assert!(store
            .store_observation(&observation("provisional:test"))
            .is_err());
        assert_eq!(
            Store::capture_recovery_status(&f.database)
                .unwrap()
                .pending_items,
            1
        );
        drop(store);
        let mut reopened = Store::open_service(&f.database).unwrap();
        assert_eq!(reopened.recover_pending_captures().unwrap().recovered, 1);
        assert!(reopened
            .contains_source("chatgpt", "provisional:test", "synthetic-source")
            .unwrap());
        assert!(!reopened
            .contains_source("chatgpt", "provisional:other", "synthetic-source")
            .unwrap());
        assert_eq!(
            reopened.recover_pending_captures().unwrap().pending_items,
            0
        );
    }
    #[test]
    fn journal_survives_before_commit_and_repeat_replay_keeps_one_chat() {
        let f = Fixture::new();
        let mut store = Store::open_service(&f.database).unwrap();
        let journal = Journal::for_database(&f.database, true).unwrap().unwrap();
        let record = observation("provisional:test");
        journal.stage(&record).unwrap();
        drop(store);
        store = Store::open_service(&f.database).unwrap();
        store.recover_pending_captures().unwrap();
        journal.stage(&record).unwrap();
        store.recover_pending_captures().unwrap();
        assert_eq!(store.library_snapshot().unwrap().conversations.len(), 1);
    }
    #[test]
    fn corrupt_and_oversized_entries_are_not_accepted_or_deleted() {
        let f = Fixture::new();
        let mut store = Store::open_service(&f.database).unwrap();
        let journal = Journal::for_database(&f.database, true).unwrap().unwrap();
        let path = journal.stage(&observation("provisional:test")).unwrap();
        fs::write(&path, b"broken").unwrap();
        assert_eq!(store.recover_pending_captures().unwrap().failed, 1);
        assert!(path.exists());
        let mut large = observation("provisional:test");
        large.title = "x".repeat(ITEM_LIMIT as usize);
        assert!(journal.stage(&large).is_err());
    }
    #[test]
    fn restored_database_does_not_replay_the_replaced_library_journal() {
        let f = Fixture::new();
        let store = Store::open(&f.database).unwrap();
        Journal::for_database(&f.database, true)
            .unwrap()
            .unwrap()
            .stage(&observation("provisional:test"))
            .unwrap();
        drop(store);
        fs::rename(&f.database, f.dir.join("old.sqlite3")).unwrap();
        let mut restored = Store::open(&f.database).unwrap();
        assert_eq!(
            restored.recover_pending_captures().unwrap().pending_items,
            0
        );
        assert_eq!(restored.library_snapshot().unwrap().conversations.len(), 0);
    }
    #[cfg(unix)]
    #[test]
    fn journal_is_private_and_rejects_symlinks() {
        use std::os::unix::fs::{symlink, PermissionsExt};
        let f = Fixture::new();
        let _store = Store::open(&f.database).unwrap();
        let journal = Journal::for_database(&f.database, true).unwrap().unwrap();
        let ticket = journal.stage(&observation("provisional:test")).unwrap();
        assert_eq!(
            fs::metadata(&journal.dir).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(&ticket).unwrap().permissions().mode() & 0o777,
            0o600
        );
        symlink(&ticket, journal.dir.join("untrusted.json")).unwrap();
        assert!(journal.entries().is_err());
    }
}
