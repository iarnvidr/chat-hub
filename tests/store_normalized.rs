use chat_hub::store::{GraphNode, Observation, Store};
use serde_json::{json, Value};

fn node(id: &str, parent: Option<&str>, role: Option<&str>, parts: Vec<Value>) -> GraphNode {
    GraphNode {
        source_node_id: id.into(),
        source_message_id: None,
        parent_id: parent.map(str::to_owned),
        role: role.map(str::to_owned),
        author: None,
        create_time: None,
        parts,
    }
}
fn obs(source: &str, graph: Vec<GraphNode>, current: Option<&str>) -> Observation {
    Observation {
        provider: "p".into(),
        account_scope: "scope".into(),
        source_id: source.into(),
        title: source.into(),
        url: None,
        current_node: current.map(str::to_owned),
        graph,
        raw_payload: json!({}),
        source_time: None,
        observed_ms: 1,
        parser: "test".into(),
        completeness: "complete".into(),
    }
}
fn store() -> Store {
    Store::in_memory().unwrap()
}
#[test]
fn normalized_nodes_messages_parts_and_time_are_distinct() {
    let mut s = store();
    let o = obs(
        "graph",
        vec![node("a", None, None, vec![json!("not a message")]), {
            let mut n = node(
                "b",
                Some("a"),
                Some("user"),
                vec![json!(""), json!({"x":1}), json!(false), json!("tail")],
            );
            n.create_time = Some(json!(1.2349));
            n
        }],
        Some("b"),
    );
    let id = s.ingest_observation(&o).unwrap();
    let v = s.read_normalized(&id).unwrap().unwrap();
    assert_eq!(v["nodes"].as_array().unwrap().len(), 2);
    assert_eq!(v["messages"].as_array().unwrap().len(), 1);
    let m = &v["messages"][0];
    assert_eq!(m["content"], "\ntail");
    assert_eq!(m["parts"][0], json!({"kind":"text","text":"","raw":""}));
    assert_eq!(m["parts"][1], json!({"kind":"unknown","raw":{"x":1}}));
    assert_eq!(m["parts"][2]["kind"], "unknown");
    assert_eq!(m["parts"][3]["text"], "tail");
    assert_eq!(m["created_at_ms"], 1234);
    assert_eq!(v["selected_node_id"], "b");
    assert_eq!(v["thread_node_ids"], json!(["a", "b"]));
    assert_eq!(v["thread_complete"], true);
}
#[test]
fn selection_fallback_invalid_pointer_dangling_and_cycles_are_safe() {
    let mut s = store();
    let id = s
        .ingest_observation(&obs(
            "fallback",
            vec![
                node("a", None, Some("user"), vec![]),
                node("z", Some("missing"), Some("assistant"), vec![]),
            ],
            Some("bogus"),
        ))
        .unwrap();
    let v = s.read_normalized(&id).unwrap().unwrap();
    assert_eq!(v["selection_reason"], "fallback_leaf");
    assert_eq!(v["selected_node_id"], "a");
    assert_eq!(v["thread_node_ids"], json!(["a"]));
    assert_eq!(v["thread_complete"], true);
    let id = s
        .ingest_observation(&obs(
            "cycle",
            vec![
                node("a", Some("b"), Some("user"), vec![]),
                node("b", Some("a"), Some("assistant"), vec![]),
            ],
            Some("a"),
        ))
        .unwrap();
    let v = s.read_normalized(&id).unwrap().unwrap();
    assert_eq!(v["thread_node_ids"], json!([]));
    assert_eq!(v["thread_complete"], false);
    assert_eq!(v["selection_reason"], "source_current_node");
    assert_eq!(s.read_normalized("missing").unwrap(), None);
}
#[test]
fn scoped_conversations_and_global_message_id_collisions_are_allocated() {
    let mut s = store();
    let mut a = obs(
        "same",
        vec![node("n", None, Some("user"), vec![json!("a")])],
        None,
    );
    let id1 = s.ingest_observation(&a).unwrap();
    a.account_scope = "other".into();
    let id2 = s.ingest_observation(&a).unwrap();
    assert_ne!(id1, id2);
    assert!(id1.starts_with("conversation:"));
    // Each normalized message links back to its own node; IDs differ by conversation scope.
    assert_eq!(
        s.read_normalized(&id1).unwrap().unwrap()["messages"][0]["node_id"],
        "n"
    );
    assert_eq!(
        s.read_normalized(&id2).unwrap().unwrap()["messages"][0]["node_id"],
        "n"
    );
    assert_ne!(id1, id2);
}
#[test]
fn migrated_legacy_message_links_are_exposed() {
    let path = std::env::temp_dir().join(format!("normalized-migration-{}.db", std::process::id()));
    let _ = std::fs::remove_file(&path);
    let c = rusqlite::Connection::open(&path).unwrap();
    c.execute_batch(include_str!("fixtures/store/v2.sql"))
        .unwrap();
    drop(c);
    let s = Store::open(path.to_str().unwrap()).unwrap();
    let v = s.read_normalized("legacy-v2").unwrap().unwrap();
    assert_eq!(v["nodes"][0]["message_id"], "legacy-message");
    assert_eq!(v["messages"][0]["id"], "legacy-message");
    drop(s);
    std::fs::remove_file(path).unwrap();
}
