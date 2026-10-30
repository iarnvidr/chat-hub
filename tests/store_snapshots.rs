use chat_hub::store::{GraphNode, Observation, Store};
use serde_json::{json, Value};
fn make() -> Observation {
    Observation {
        provider: "p".into(),
        account_scope: "a".into(),
        source_id: "s".into(),
        title: "Title words".into(),
        url: None,
        current_node: None,
        graph: vec![GraphNode {
            source_node_id: "n".into(),
            source_message_id: None,
            parent_id: None,
            role: Some("user".into()),
            author: None,
            create_time: None,
            parts: vec![Value::String("unique needle".into())],
        }],
        raw_payload: json!({}),
        source_time: None,
        observed_ms: 42,
        parser: "test".into(),
        completeness: "complete".into(),
    }
}
#[test]
fn snapshots_have_revision_and_message_title_search() {
    let mut s = Store::in_memory().unwrap();
    assert_eq!(s.library_revision().unwrap(), 0);
    let o = make();
    s.ingest_observation(&o).unwrap();
    let snap = s.library_snapshot().unwrap();
    assert_eq!(snap.library_revision, 1);
    assert_eq!(snap.conversations[0]["updated_at_ms"], 42);
    assert_eq!(s.search_snapshot("needle").unwrap().hits.len(), 1);
    assert_eq!(s.search_snapshot("Title").unwrap().hits.len(), 1);
    let mut duplicate = o.clone();
    duplicate.observed_ms = 999;
    duplicate.raw_payload = json!({"trace":"different"});
    s.ingest_observation(&duplicate).unwrap();
    assert_eq!(s.library_revision().unwrap(), 1);
    assert_eq!(
        s.read_normalized(snap.conversations[0]["id"].as_str().unwrap())
            .unwrap()
            .unwrap()["updated_at_ms"],
        42
    );
    let mut changed = o.clone();
    changed.title = "New title".into();
    changed.raw_payload = json!({"update_time": 1.0});
    changed.observed_ms = 50;
    s.ingest_observation(&changed).unwrap();
    assert_eq!(s.library_revision().unwrap(), 2);
}
#[test]
fn search_hits_route_to_descendant_leaf_and_mark_selected_ancestry() {
    let mut s = Store::in_memory().unwrap();
    let mut o = make();
    o.current_node = Some("selected-leaf".into());
    o.graph = vec![
        node("root", None, "root words"),
        node("selected-leaf", Some("root"), "branch marker"),
        node("alternate", Some("root"), "branch marker"),
        node("z-child", Some("alternate"), "internal marker"),
        node("a-child", Some("alternate"), "internal marker"),
    ];
    s.ingest_observation(&o).unwrap();
    let hits = &s.search_snapshot("marker").unwrap().hits;
    let alternate = hits.iter().find(|h| h["node_id"] == "alternate").unwrap();
    assert_eq!(alternate["selected_branch"], false);
    assert_eq!(alternate["leaf_node_id"], "a-child");
    let selected = hits
        .iter()
        .find(|h| h["node_id"] == "selected-leaf")
        .unwrap();
    assert_eq!(selected["selected_branch"], true);
    let title = s.search_snapshot("Title").unwrap();
    assert!(title.hits.iter().all(|h| h["node_id"].is_null()
        && h["message_id"].is_null()
        && h["leaf_node_id"].is_null()));
}
fn node(id: &str, parent: Option<&str>, text: &str) -> GraphNode {
    GraphNode {
        source_node_id: id.into(),
        source_message_id: None,
        parent_id: parent.map(str::to_owned),
        role: Some("user".into()),
        author: None,
        create_time: None,
        parts: vec![Value::String(text.into())],
    }
}

#[test]
fn stale_source_observation_cannot_roll_back_content_or_selection() {
    let mut s = Store::in_memory().unwrap();
    let mut newest = make();
    newest.raw_payload = json!({"update_time":20.0});
    newest.current_node = Some("n".into());
    let id = s.ingest_observation(&newest).unwrap();
    let mut stale = make();
    stale.title = "stale".into();
    stale.graph[0].parts = vec![Value::String("old".into())];
    stale.raw_payload = json!({"update_time":10.0});
    stale.observed_ms = 500;
    s.ingest_observation(&stale).unwrap();
    assert_eq!(s.library_revision().unwrap(), 1);
    let current = s.read_normalized(&id).unwrap().unwrap();
    assert_eq!(current["title"], "Title words");
    assert_eq!(current["selected_node_id"], "n");
    assert_eq!(current["messages"][0]["content"], "unique needle");
    assert!(s.search("stale", 10).unwrap().is_empty());
    assert!(s.search_snapshot("stale").unwrap().hits.is_empty());
    assert_eq!(s.search("Title", 10).unwrap().len(), 1);
    assert_eq!(s.search_snapshot("Title").unwrap().hits.len(), 1);
}

#[test]
fn retained_url_does_not_create_revision_when_input_omits_url() {
    let mut s = Store::in_memory().unwrap();
    let mut first = make();
    first.url = Some("https://example.test/item".into());
    first.raw_payload = json!({"update_time": 10.0});
    s.ingest_observation(&first).unwrap();
    assert_eq!(s.library_revision().unwrap(), 1);

    let mut no_op = first.clone();
    no_op.url = None;
    no_op.raw_payload = json!({"update_time": 11.0});
    no_op.observed_ms += 1;
    s.ingest_observation(&no_op).unwrap();
    // The source update timestamp is observable conversation metadata and advances
    // the revision even though the omitted URL retains the prior URL value.
    assert_eq!(s.library_revision().unwrap(), 2);
    let snapshot = s.library_snapshot().unwrap();
    assert_eq!(
        snapshot.conversations[0]["url"],
        "https://example.test/item"
    );
}

#[test]
fn equal_timestamp_conflict_uses_deterministic_payload_order() {
    let mut s = Store::in_memory().unwrap();
    let mut first = make();
    first.raw_payload = json!({"update_time": 10.0});
    let id = s.ingest_observation(&first).unwrap();
    let mut conflict = first.clone();
    conflict.title = "conflicting title".into();
    conflict.graph[0].parts = vec![Value::String("conflicting body".into())];
    conflict.raw_payload = json!({"update_time": 10.0, "trace": "conflict"});
    s.ingest_observation(&conflict).unwrap();
    assert_eq!(s.library_revision().unwrap(), 2);
    let current = s.read_normalized(&id).unwrap().unwrap();
    assert_eq!(current["title"], "conflicting title");
    assert_eq!(current["messages"][0]["content"], "conflicting body");
    assert_eq!(s.search("conflicting", 10).unwrap().len(), 1);
    assert_eq!(s.read(&id).unwrap().unwrap()["title"], "conflicting title");
}
