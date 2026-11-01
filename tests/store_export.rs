mod common;

use chat_hub::store::{
    ConversationStateChange, GraphNode, Observation, Store, VerifiedConversationStateReceipt,
};
use serde_json::{json, Value};
use std::io::{self, Write};

fn observation() -> Observation {
    Observation {
        provider: "chatgpt".into(),
        account_scope: format!("chatgpt:observed:{}", "e".repeat(64)),
        source_id: "origin".into(),
        title: "wide".into(),
        url: Some("https://example.invalid".into()),
        current_node: Some("node".into()),
        graph: vec![GraphNode {
            source_node_id: "node".into(),
            source_message_id: Some("source-msg".into()),
            parent_id: None,
            role: Some("user".into()),
            author: Some(json!({"speaker":"user","x":1})),
            create_time: Some(json!(1.25)),
            parts: vec![
                Value::String("z".repeat(9 * 1024 * 1024)),
                json!({"unknown":true}),
            ],
        }],
        raw_payload: json!({"payload":[null,3]}),
        source_time: Some("original".into()),
        observed_ms: 42,
        parser: "fixture".into(),
        completeness: "complete".into(),
    }
}
#[test]
fn deterministic_lossless_streamed_rows_and_no_clobber() {
    let mut store = Store::in_memory().unwrap();
    let conversation_id = store.ingest_observation(&observation()).unwrap();
    store
        .set_target_tags(
            "conversation",
            &conversation_id,
            &["Research".into(), "MVP".into()],
        )
        .unwrap();
    store
        .set_target_tags("entity", "entity:repository-digest", &["Research".into()])
        .unwrap();
    let state_scope = store
        .conversation_state_identity(&conversation_id)
        .unwrap()
        .unwrap()
        .account_scope;
    store
        .commit_verified_conversation_state(
            &conversation_id,
            &VerifiedConversationStateReceipt {
                verified: true,
                conversation_id: conversation_id.clone(),
                source_conversation_id: "origin".into(),
                scope: state_scope.clone(),
                target_provider: "chatgpt".into(),
                target_scope: state_scope,
                archived: Some(false),
                pinned: Some(true),
            },
            ConversationStateChange {
                archived: Some(false),
                pinned: Some(true),
            },
        )
        .unwrap();
    let mut a = Vec::new();
    let meta = store.write_library_json(&mut a).unwrap();
    let mut b = Vec::new();
    store.write_library_json(&mut b).unwrap();
    assert_eq!(a, b);
    assert_eq!(meta.node_count, 1);
    assert_eq!(meta.observation_count, 1);
    let doc: Value = serde_json::from_slice(&a).unwrap();
    assert_eq!(doc["format"], "chat-hub-library");
    assert_eq!(doc["tags"].as_array().unwrap().len(), 2);
    assert_eq!(doc["target_tags"].as_array().unwrap().len(), 3);
    assert_eq!(doc["target_tags"][0]["target_kind"], "conversation");
    assert_eq!(doc["conversation_state"].as_array().unwrap().len(), 1);
    assert_eq!(doc["conversation_state"][0]["pinned"], 1);
    assert_eq!(doc["conversation_state"][0]["archived"], 0);
    assert!(doc["nodes"][0]["parts_json"]
        .as_str()
        .unwrap()
        .contains("unknown"));
    let raw_parts: Value =
        serde_json::from_str(doc["nodes"][0]["parts_json"].as_str().unwrap()).unwrap();
    assert_eq!(raw_parts[0].as_str().unwrap(), "z".repeat(9 * 1024 * 1024));
    assert_eq!(raw_parts[1], json!({"unknown":true}));
    assert_eq!(doc["nodes"][0]["parent_id"], Value::Null);
    assert_eq!(doc["nodes"][0]["role"], "user");
    assert_eq!(
        doc["nodes"][0]["author_json"],
        "{\"speaker\":\"user\",\"x\":1}"
    );
    assert_eq!(doc["nodes"][0]["create_time_json"], "1.25");
    assert_eq!(doc["conversations"][0]["current_node"], "node");
    assert_eq!(doc["observations"][0]["source_time"], "original");
    assert_eq!(doc["observations"][0]["parser"], "fixture");
    assert_eq!(doc["observations"][0]["completeness"], "complete");
    assert_eq!(
        doc["observations"][0]["raw_payload"],
        "{\"payload\":[null,3]}"
    );
    let path = common::temp_dir("store-export-json").join("export.json");
    let receipt = store.export_json_to(&path).unwrap();
    assert_eq!(receipt.metadata, meta);
    assert!(store.export_json_to(&path).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), a);
    std::fs::remove_file(path).unwrap();
}
struct Failing;
impl Write for Failing {
    fn write(&mut self, _: &[u8]) -> io::Result<usize> {
        Err(io::Error::other("failed"))
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
#[test]
fn export_keeps_one_snapshot_while_wal_writer_commits() {
    use std::sync::{Arc, Barrier};
    use std::thread;

    let db = common::temp_dir("export-snapshot").join("snapshot.sqlite");
    let path = db.to_str().unwrap();
    let conn = rusqlite::Connection::open(path).unwrap();
    conn.execute_batch("PRAGMA journal_mode=WAL;").unwrap();
    drop(conn);
    let mut setup = Store::open(path).unwrap();
    setup.ingest_observation(&observation()).unwrap();
    let before = setup.library_revision().unwrap();
    drop(setup);

    let entered = Arc::new(Barrier::new(2));
    let resume = Arc::new(Barrier::new(2));
    let export_path = path.to_owned();
    let e = entered.clone();
    let r = resume.clone();
    let exporting = thread::spawn(move || {
        let store = Store::open(&export_path).unwrap();
        let mut writer = BlockingWriter {
            bytes: Vec::new(),
            entered: e,
            resume: r,
            blocked: false,
        };
        let metadata = store.write_library_json(&mut writer).unwrap();
        (metadata, writer.bytes)
    });

    // The export's first write occurs after metadata queries have pinned its read transaction.
    entered.wait();
    let mut writer = Store::open(path).unwrap();
    let mut changed = observation();
    changed.title = "post-write title".into();
    changed.observed_ms = 43;
    changed.graph[0].parts = vec![json!("new row")];
    writer.ingest_observation(&changed).unwrap();
    let mut added = observation();
    added.source_id = "second".into();
    added.title = "second conversation".into();
    writer.ingest_observation(&added).unwrap();
    let after = writer.library_revision().unwrap();
    assert!(after > before);
    resume.wait();

    let (meta, bytes) = exporting.join().unwrap();
    let document: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(meta.library_revision, before);
    assert_eq!(meta.conversation_count, 1);
    assert_eq!(meta.node_count, 1);
    assert_eq!(meta.observation_count, 1);
    assert_eq!(document["conversations"].as_array().unwrap().len(), 1);
    assert_eq!(document["conversations"][0]["title"], "wide");
    assert_eq!(
        document["nodes"][0]["parts_json"],
        serde_json::to_string(&observation().graph[0].parts).unwrap()
    );
    assert_eq!(document["observations"].as_array().unwrap().len(), 1);
    assert_eq!(writer.library_revision().unwrap(), after);
    let check = rusqlite::Connection::open(path).unwrap();
    assert_eq!(
        check
            .query_row("SELECT count(*) FROM conversations", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        2
    );
    assert_eq!(
        check
            .query_row(
                "SELECT title FROM conversations WHERE source_id='origin'",
                [],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
        "post-write title"
    );
    drop(check);
    drop(writer);
    let _ = std::fs::remove_file(&db);
    let _ = std::fs::remove_file(format!("{path}-wal"));
    let _ = std::fs::remove_file(format!("{path}-shm"));
}

struct BlockingWriter {
    bytes: Vec<u8>,
    entered: std::sync::Arc<std::sync::Barrier>,
    resume: std::sync::Arc<std::sync::Barrier>,
    blocked: bool,
}
impl Write for BlockingWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if !self.blocked {
            self.blocked = true;
            self.entered.wait();
            self.resume.wait();
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn writer_failure_is_reported() {
    let store = Store::in_memory().unwrap();
    assert!(store.write_library_json(&mut Failing).is_err());
}
