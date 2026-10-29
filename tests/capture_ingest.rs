use chat_hub::{proxy::CaptureEvent, store::Store};

#[test]
fn captured_synthetic_conversation_ingests_and_rereads_idempotently() {
    let mut store = Store::in_memory().unwrap();
    let body = br#"{"title":"Synthetic acceptance","mapping":{"u":{"parent":null,"message":{"author":{"role":"user"},"content":{"parts":["capture-proof-user"]}}},"a":{"parent":"u","message":{"author":{"role":"assistant"},"content":{"parts":["capture-proof-assistant"]}}}}}"#.to_vec();
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
