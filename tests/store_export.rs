use chat_hub::store::{GraphNode, Observation, Store};
use serde_json::{json, Value};
use std::io::{self, Write};

fn observation() -> Observation {
    Observation {
        provider: "provider".into(),
        account_scope: "acct".into(),
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
    store.ingest_observation(&observation()).unwrap();
    let mut a = Vec::new();
    let meta = store.write_library_json(&mut a).unwrap();
    let mut b = Vec::new();
    store.write_library_json(&mut b).unwrap();
    assert_eq!(a, b);
    assert_eq!(meta.node_count, 1);
    assert_eq!(meta.observation_count, 1);
    let doc: Value = serde_json::from_slice(&a).unwrap();
    assert_eq!(doc["format"], "chat-hub-library");
    assert!(doc["nodes"][0]["parts_json"]
        .as_str()
        .unwrap()
        .contains("unknown"));
    assert_eq!(
        doc["observations"][0]["raw_payload"],
        "{\"payload\":[null,3]}"
    );
    let path = std::env::temp_dir().join(format!(
        "export-{}-{}.json",
        std::process::id(),
        meta.library_revision
    ));
    let _ = std::fs::remove_file(&path);
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
fn writer_failure_is_reported() {
    let store = Store::in_memory().unwrap();
    assert!(store.write_library_json(&mut Failing).is_err());
}
