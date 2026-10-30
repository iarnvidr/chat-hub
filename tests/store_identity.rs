use chat_hub::store::{GraphNode, Observation, Store};
use serde_json::{json, Value};

fn observation(
    provider: &str,
    scope: &str,
    source: &str,
    node: &str,
    message: &str,
) -> Observation {
    Observation {
        provider: provider.into(),
        account_scope: scope.into(),
        source_id: source.into(),
        title: source.into(),
        url: None,
        current_node: Some(node.into()),
        graph: vec![GraphNode {
            source_node_id: node.into(),
            source_message_id: Some(message.into()),
            parent_id: None,
            role: Some("user".into()),
            author: None,
            create_time: None,
            parts: vec![Value::String(format!("content-{source}"))],
        }],
        raw_payload: json!({}),
        source_time: None,
        observed_ms: 1,
        parser: "test".into(),
        completeness: "complete".into(),
    }
}

#[test]
fn repeated_node_and_provider_message_ids_are_conversation_scoped() {
    let mut store = Store::in_memory().unwrap();
    let a = store
        .ingest_observation(&observation("p", "s", "a", "same-node", "same-message"))
        .unwrap();
    let b = store
        .ingest_observation(&observation("p", "s", "b", "same-node", "same-message"))
        .unwrap();
    assert_ne!(a, b);
    assert_eq!(
        store.read(&a).unwrap().unwrap()["nodes"][0]["id"],
        "same-node"
    );
    assert_eq!(
        store.read(&b).unwrap().unwrap()["nodes"][0]["source_message_id"],
        "same-message"
    );
}

#[test]
fn tuple_delimiters_cannot_collide() {
    let mut store = Store::in_memory().unwrap();
    let a = store
        .ingest_observation(&observation("a:b", "c", "d", "n", "m"))
        .unwrap();
    let b = store
        .ingest_observation(&observation("a", "b:c", "d", "n", "m"))
        .unwrap();
    assert_ne!(a, b);
}
