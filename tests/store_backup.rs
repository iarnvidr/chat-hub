mod common;

use chat_hub::{
    archive::BackupReceipt,
    store::{GraphNode, Observation, Store},
};
use rusqlite::Connection;
use serde_json::{json, Value};
use std::{
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Barrier,
    },
    thread,
    time::{Duration, Instant},
};

fn observation(
    source: &str,
    title: &str,
    graph: Vec<GraphNode>,
    current: Option<&str>,
) -> Observation {
    Observation {
        provider: "backup-test".into(),
        account_scope: "scope".into(),
        source_id: source.into(),
        title: title.into(),
        url: None,
        current_node: current.map(str::to_owned),
        graph,
        raw_payload: json!({"source": source}),
        source_time: None,
        observed_ms: 10,
        parser: "acceptance".into(),
        completeness: "complete".into(),
    }
}
fn node(id: &str, parent: Option<&str>, parts: Vec<Value>) -> GraphNode {
    GraphNode {
        source_node_id: id.into(),
        source_message_id: Some(format!("msg-{id}")),
        parent_id: parent.map(str::to_owned),
        role: Some("assistant".into()),
        author: None,
        create_time: None,
        parts,
    }
}
fn check_database(path: &std::path::Path) -> Connection {
    let db = Connection::open(path).unwrap();
    assert_eq!(
        db.query_row("PRAGMA integrity_check", [], |r| r.get::<_, String>(0))
            .unwrap(),
        "ok"
    );
    let fk: i64 = db
        .query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(fk, 0);
    db
}

#[test]
fn backup_is_independent_valid_and_never_overwrites() {
    let dir = common::temp_dir("chat-hub-backup-basic");
    let source = dir.join("source.db");
    let dest = dir.join("backup.db");
    let mut store = Store::open(source.to_str().unwrap()).unwrap();
    let id1 = store
        .ingest_observation(&observation(
            "branches",
            "Branch-searchable-title",
            vec![
                node("root", None, vec![json!("root phrase")]),
                node(
                    "left",
                    Some("root"),
                    vec![json!("branch searchable content")],
                ),
                node(
                    "right",
                    Some("root"),
                    vec![
                        json!("alternate text"),
                        json!({"unknown":{"nested":[1,true]}}),
                    ],
                ),
                node(
                    "orphan",
                    Some("absent-parent"),
                    vec![json!("orphan branch")],
                ),
            ],
            Some("left"),
        ))
        .unwrap();
    let id2 = store
        .ingest_observation(&observation(
            "null-root",
            "Null root searchable title",
            vec![
                node("one", None, vec![json!("null-root content")]),
                node("two", None, vec![json!("separate root")]),
            ],
            None,
        ))
        .unwrap();
    let receipt: BackupReceipt = store.backup_to(&dest).unwrap();
    assert_eq!(receipt.schema_version, store.schema_version().unwrap());
    assert_eq!(receipt.library_revision, store.library_revision().unwrap());
    let copy = check_database(&dest);
    assert_eq!(
        copy.query_row("SELECT count(*) FROM conversations", [], |r| r
            .get::<_, u64>(0))
            .unwrap(),
        receipt.conversation_count
    );
    assert_eq!(receipt.conversation_count, 2);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&dest).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    assert_eq!(
        copy.query_row("SELECT count(*) FROM observations", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        2
    );
    drop(copy);
    drop(store);
    let reopened = Store::open(dest.to_str().unwrap()).unwrap();
    assert_eq!(
        reopened.library_revision().unwrap(),
        receipt.library_revision
    );
    assert_eq!(
        reopened.search("branch searchable", 10).unwrap(),
        vec![id1.clone()]
    );
    assert_eq!(reopened.search("null-root content", 10).unwrap(), vec![id2]);
    let graph = reopened.read_normalized(&id1).unwrap().unwrap();
    assert_eq!(graph["thread_node_ids"], json!(["root", "left"]));
    assert_eq!(graph["nodes"].as_array().unwrap().len(), 4);
    assert_eq!(graph["messages"][2]["parts"][1]["kind"], "unknown");
    drop(reopened);
    let store = Store::open(source.to_str().unwrap()).unwrap();
    assert!(store.backup_to(&dest).is_err());
    assert!(store.backup_to(&source).is_err());
    let missing = dir.join("absent").join("backup.db");
    assert!(store.backup_to(&missing).is_err());
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 2);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn populated_backup_is_a_consistent_snapshot_during_wal_writes() {
    let dir = common::temp_dir("wal-concurrent");
    let source = dir.join("source.db");
    let dest = dir.join("backup.db");
    let mut store = Store::open(source.to_str().unwrap()).unwrap();
    store
        .ingest_observation(&observation(
            "paired",
            "pair-0",
            vec![node("n", None, vec![json!("wal snapshot marker")])],
            Some("n"),
        ))
        .unwrap();
    let setup = Connection::open(&source).unwrap();
    setup.pragma_update(None, "journal_mode", "WAL").unwrap();
    drop(setup);
    let started = Arc::new(Barrier::new(2));
    let stop = Arc::new(AtomicBool::new(false));
    let commits = Arc::new(AtomicUsize::new(0));
    let writer = {
        let started = started.clone();
        let stop = stop.clone();
        let commits = commits.clone();
        let path = source.clone();
        thread::spawn(move || {
            let mut writer = Store::open(path.to_str().unwrap()).unwrap();
            started.wait();
            let mut seq = 0u64;
            while !stop.load(Ordering::Acquire) {
                seq += 1;
                let title = format!("pair-{seq}");
                writer
                    .ingest_observation(&observation(
                        "paired",
                        &title,
                        vec![node(
                            "n",
                            None,
                            vec![json!(format!("wal snapshot marker revision-{seq}"))],
                        )],
                        Some("n"),
                    ))
                    .unwrap();
                commits.fetch_add(1, Ordering::Release);
                thread::yield_now();
            }
        })
    };
    started.wait();
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while commits.load(Ordering::Acquire) < 5 && std::time::Instant::now() < deadline {
        thread::sleep(Duration::from_millis(2));
    }
    let before = commits.load(Ordering::Acquire);
    assert!(before >= 5, "writer must commit before backup");
    let receipt: BackupReceipt = store.backup_to(&dest).unwrap();
    let during = commits.load(Ordering::Acquire);
    stop.store(true, Ordering::Release);
    writer.join().unwrap();
    let after = commits.load(Ordering::Acquire);
    assert!(
        after > before,
        "writer must commit after snapshot begins/completes"
    );
    let copy = check_database(&dest);
    assert_eq!(
        copy.query_row("SELECT count(*) FROM conversations", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
    let (title, revision): (String, i64) = copy
        .query_row(
            "SELECT c.title,l.revision FROM conversations c CROSS JOIN library_state l",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(revision, receipt.library_revision);
    let seq: u64 = title.strip_prefix("pair-").unwrap().parse().unwrap();
    let content: String = copy
        .query_row("SELECT content FROM graph_nodes", [], |r| r.get(0))
        .unwrap();
    assert_eq!(
        content,
        format!("wal snapshot marker revision-{seq}"),
        "title/content must come from the same committed ingest transaction"
    );
    assert_eq!(
        copy.query_row("SELECT count(*) FROM observations", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        revision,
        "revision and transactionally recorded observation count must agree"
    );
    drop(copy);
    drop(store);
    std::fs::remove_dir_all(dir).unwrap();
    let _ = (during, after);
}

#[cfg(unix)]
#[test]
fn backup_rejects_existing_aliases_and_cleans_failed_staging() {
    use std::os::unix::fs::symlink;
    let dir = common::temp_dir("aliases");
    let source = dir.join("source.db");
    let dest = dir.join("backup.db");
    let store = Store::open(source.to_str().unwrap()).unwrap();
    let marker = dir.join("marker");
    std::fs::write(&marker, b"do not overwrite").unwrap();
    symlink(&marker, &dest).unwrap();
    assert!(store.backup_to(&dest).is_err());
    assert_eq!(std::fs::read(&marker).unwrap(), b"do not overwrite");
    std::fs::remove_file(&dest).unwrap();
    std::fs::hard_link(&source, &dest).unwrap();
    assert!(store.backup_to(&dest).is_err());
    std::fs::remove_file(&dest).unwrap();
    let missing = dir.join("missing-parent").join("backup.db");
    assert!(store.backup_to(&missing).is_err());
    assert_eq!(
        std::fs::read_dir(&dir).unwrap().count(),
        2,
        "failed paths leave no staging artifacts"
    );
    drop(store);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn backup_busy_deadline_is_bounded_and_cleans_staging() {
    use chat_hub::archive::backup_to_completion;
    use rusqlite::backup::Backup;
    let dir = common::temp_dir("busy-deadline");
    let source = dir.join("source.db");
    let store = Store::open(source.to_str().unwrap()).unwrap();
    let blocker = Connection::open(&source).unwrap();
    blocker.execute_batch("BEGIN EXCLUSIVE;").unwrap();
    let source_connection = Connection::open(&source).unwrap();
    source_connection.busy_timeout(Duration::ZERO).unwrap();
    let dest = dir.join("backup.db");
    std::fs::write(&dest, b"preserved database").unwrap();
    let start = Instant::now();
    let result = {
        let mut target = Connection::open_in_memory().unwrap();
        let backup = Backup::new(&source_connection, &mut target).unwrap();
        backup_to_completion(
            &backup,
            128,
            Duration::from_millis(5),
            Duration::from_millis(50),
        )
    };
    assert!(matches!(
        result,
        Err(chat_hub::error::Error::Configuration(message))
            if message == "database backup busy timeout elapsed"
                || message == "database backup deadline elapsed"
    ));
    assert!(start.elapsed() < Duration::from_secs(2));
    assert_eq!(std::fs::read(&dest).unwrap(), b"preserved database");
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 2);
    blocker.execute_batch("ROLLBACK;").unwrap();
    drop(source_connection);
    drop(blocker);
    drop(store);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn atomic_output_destination_race_preserves_target_and_cleans_stage() {
    use chat_hub::archive::AtomicOutput;
    let dir = common::temp_dir("publication-race");
    let dest = dir.join("backup.db");
    let mut output = AtomicOutput::new(&dest).unwrap();
    std::io::Write::write_all(output.file(), b"complete staged bytes").unwrap();
    let stage = output.staging_path().to_path_buf();
    std::fs::write(&dest, b"existing target").unwrap();
    assert!(output.finish().is_err());
    assert_eq!(std::fs::read(&dest).unwrap(), b"existing target");
    assert!(!stage.exists());
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
    std::fs::remove_dir_all(dir).unwrap();
}
