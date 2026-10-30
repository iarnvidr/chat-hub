use chat_hub::{
    capture::{normalize_chatgpt_document, parse_event},
    proxy::CaptureEvent,
    store::{IngestOutcome, Store},
};
use serde_json::json;

#[test]
fn eligible_non_object_and_missing_mapping_documents_are_ignored_without_panicking() {
    for body in [
        br#"null"#.as_slice(),
        br#"[]"#,
        br#"\"text\""#,
        br#"42"#,
        br#"{}"#,
    ] {
        let event = CaptureEvent {
            host: "chatgpt.com".into(),
            path: "/backend-api/conversation/scalar".into(),
            status: 200,
            body: body.to_vec(),
        };
        assert!(parse_event(&event).is_none());
    }
}
#[test]
fn captured_synthetic_conversation_ingests_and_rereads_idempotently() {
    let mut store = Store::in_memory().unwrap();
    let body=br#"{"title":"Synthetic acceptance","mapping":{"u":{"parent":null,"message":{"author":{"role":"user"},"content":{"parts":["capture-proof-user"]}}},"a":{"parent":"u","message":{"author":{"role":"assistant"},"content":{"parts":["capture-proof-assistant"]}}}}}"#.to_vec();
    let event = || CaptureEvent {
        host: "chatgpt.com".into(),
        path: "/backend-api/conversation/synthetic-id".into(),
        status: 200,
        body: body.clone(),
    };
    assert!(store.ingest_capture(event()).unwrap());
    assert!(store.ingest_capture(event()).unwrap());
    let read = store.read("synthetic-id").unwrap().unwrap();
    assert_eq!(read["provider"], "chatgpt");
    assert_eq!(read["messages"].as_array().unwrap().len(), 2);
    assert!(read.to_string().contains("capture-proof-assistant"));
    assert!(!store
        .ingest_capture(CaptureEvent {
            host: "chatgpt.com".into(),
            path: "/backend-api/models".into(),
            status: 200,
            body: b"{}".to_vec()
        })
        .unwrap());
}
#[test]
fn export_id_and_live_documents_share_normalization_safely() {
    let doc = json!({"id":"bad/id?x=1","title":"Export","update_time":3.5,"current_node":"n","mapping":{"n":{"parent":null,"message":{"id":"m","author":{"role":"user"},"create_time":2.0,"content":{"parts":[{"x":1},null]}}},"empty":{"parent":"n","message":null}}});
    let export = normalize_chatgpt_document(&doc, "chatgpt-export-v1", 42).unwrap();
    assert_eq!(export.source_id, "bad/id?x=1");
    assert!(export.url.as_ref().unwrap().contains("bad%2Fid%3Fx%3D1"));
    assert_eq!(export.parser, "chatgpt-export-v1");
    assert_eq!(
        export
            .graph
            .iter()
            .find(|n| n.source_node_id == "n")
            .unwrap()
            .parts
            .len(),
        2
    );
    assert_eq!(
        export
            .graph
            .iter()
            .find(|n| n.source_node_id == "empty")
            .unwrap()
            .parts
            .len(),
        0
    );
    assert_eq!(export.observed_ms, 42);
    assert_eq!(export.source_time.as_deref(), Some("3.5"));
    let mut live = doc.clone();
    live.as_object_mut()
        .unwrap()
        .insert("conversation_id".into(), json!("bad/id?x=1"));
    let live = normalize_chatgpt_document(&live, "chatgpt-live-v1", 99).unwrap();
    assert_eq!(export.source_id, live.source_id);
    assert_eq!(
        export
            .graph
            .iter()
            .find(|n| n.source_node_id == "n")
            .unwrap()
            .parts,
        live.graph
            .iter()
            .find(|n| n.source_node_id == "n")
            .unwrap()
            .parts
    );
    assert_eq!(export.url, live.url);
    let mut store = Store::in_memory().unwrap();
    assert!(matches!(
        store.ingest_observation_outcome(&export).unwrap(),
        IngestOutcome::Imported { .. }
    ));
}
