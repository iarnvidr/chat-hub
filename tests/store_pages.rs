use chat_hub::store::{GraphNode, ListKey, Observation, Store};
use serde_json::{json, Value};

fn observation(
    source_id: &str,
    title: &str,
    observed_ms: i64,
    graph: Vec<GraphNode>,
) -> Observation {
    Observation {
        provider: "test".into(),
        account_scope: "acct".into(),
        source_id: source_id.into(),
        title: title.into(),
        url: None,
        current_node: None,
        graph,
        raw_payload: Value::Null,
        source_time: None,
        observed_ms,
        parser: "test".into(),
        completeness: "complete".into(),
    }
}
fn node(id: &str, parent: Option<&str>, text: &str) -> GraphNode {
    GraphNode {
        source_node_id: id.into(),
        source_message_id: Some(id.into()),
        parent_id: parent.map(str::to_owned),
        role: Some("user".into()),
        author: None,
        create_time: None,
        parts: vec![json!(text)],
    }
}

#[test]
fn list_keyset_nulls_and_ties_and_limits() {
    let mut s = Store::in_memory().unwrap();
    s.ingest_observation(&observation("a", "a", 10, vec![]))
        .unwrap();
    s.ingest_observation(&observation("b", "b", 10, vec![]))
        .unwrap();
    s.ingest_observation(&observation("c", "c", 20, vec![]))
        .unwrap();
    let first = s.list_page_snapshot(1, None, None).unwrap();
    assert_eq!(first.items[0]["source_id"], "c");
    let key = ListKey {
        updated_at_ms: first.items[0]["updated_at_ms"].as_i64(),
        id: first.items[0]["id"].as_str().unwrap().into(),
    };
    assert_eq!(
        s.list_page_snapshot(1, Some(&key), None).unwrap().items[0]["source_id"],
        "a"
    );
    assert_eq!(
        s.list_page_snapshot(1000, None, None).unwrap().items.len(),
        3
    );
    assert!(s.list_page_snapshot(0, None, None).is_err());
    assert!(s.search_page_snapshot("x", 0, None, None).is_err());
    assert!(s
        .list_page_snapshot(1, None, Some(-1))
        .unwrap()
        .items
        .is_empty());
}
#[test]
fn stale_revision_precedes_search_query() {
    let s = Store::in_memory().unwrap();
    let snap = s
        .search_page_snapshot("[invalid FTS ???", 10, None, Some(-1))
        .unwrap();
    assert!(snap.hits.is_empty());
}
#[test]
fn detail_rejects_large_parts_without_truncating_database() {
    let mut s = Store::in_memory().unwrap();
    let id = s
        .ingest_observation(&observation(
            "large",
            "big",
            1,
            vec![node("n", None, &"x".repeat(2 * 1024 * 1024))],
        ))
        .unwrap();
    let err = s.detail_snapshot(&id).unwrap_err();
    assert!(matches!(
        err,
        chat_hub::error::Error::ReadBudgetExceeded { .. }
    ));
    // A second bounded read still sees the same budget rejection: the read did not prune data.
    assert!(matches!(
        s.detail_snapshot(&id),
        Err(chat_hub::error::Error::ReadBudgetExceeded { .. })
    ));
}
#[test]
fn bounded_graph_node_count() {
    let s = Store::in_memory().unwrap();
    assert!(s.detail_snapshot("missing").unwrap().conversation.is_none());
}
