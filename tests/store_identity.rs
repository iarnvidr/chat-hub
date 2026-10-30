mod common;

use chat_hub::store::{GraphNode, Observation, Store};
use rusqlite::Connection;
use serde_json::{json, Value};

fn db_path() -> String {
    common::temp_dir("store-identity")
        .join("library.db")
        .to_string_lossy()
        .into_owned()
}

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
    let path = db_path();
    let mut store = Store::open(&path).unwrap();
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
fn generated_ids_do_not_take_legacy_ownership() {
    let path = db_path();
    let canonical_conv = format!("conversation:{}", json!(["p", "s", "new"]));
    let canonical_msg = format!("message:{}", json!([canonical_conv, "node"]));
    let seed = Connection::open(&path).unwrap();
    seed.execute_batch("CREATE TABLE conversations(id TEXT PRIMARY KEY,title TEXT NOT NULL DEFAULT '',provider TEXT NOT NULL DEFAULT '',url TEXT,updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP); CREATE TABLE messages(id TEXT PRIMARY KEY,conversation_id TEXT NOT NULL REFERENCES conversations(id),parent_id TEXT,role TEXT NOT NULL,content TEXT NOT NULL DEFAULT '');").unwrap();
    seed.execute(
        "INSERT INTO conversations(id,title,provider) VALUES(?1,'preserve','legacy')",
        [&canonical_conv],
    )
    .unwrap();
    seed.execute(
        "INSERT INTO messages(id,conversation_id,role,content) VALUES(?1,?2,'user','keep-message')",
        [&canonical_msg, &canonical_conv],
    )
    .unwrap();
    drop(seed);
    let mut store = Store::open(&path).unwrap();
    let target = observation("p", "s", "new", "node", "m");
    let id = store.ingest_observation(&target).unwrap();
    assert_ne!(id, canonical_conv);
    let content: String = Connection::open(&path)
        .unwrap()
        .query_row(
            "SELECT content FROM messages WHERE id=?1",
            [&canonical_msg],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(content, "keep-message");
    drop(store);
    let count: i64 = Connection::open(&path)
        .unwrap()
        .query_row(
            "SELECT count(*) FROM messages WHERE conversation_id=?1",
            [&id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);
    let mut store = Store::open(&path).unwrap();
    assert_eq!(store.ingest_observation(&target).unwrap(), id);
}

#[test]
fn same_conversation_message_collision_preserves_legacy_owner_and_allocates_parent() {
    let path = db_path();
    let mut store = Store::open(&path).unwrap();
    let conv = store
        .ingest_observation(&observation("p", "s", "base", "base-node", "base-message"))
        .unwrap();
    drop(store);

    let candidate = format!("message:{}", json!([conv, "parent"]));
    let seed = Connection::open(&path).unwrap();
    seed.execute(
        "INSERT INTO messages(id,conversation_id,parent_id,role,content) VALUES(?1,?2,NULL,'user','legacy-owned')",
        [&candidate, &conv],
    )
    .unwrap();
    seed.execute(
        "INSERT INTO graph_nodes(conversation_id,source_node_id,source_message_id,message_id,parent_id,role,author_json,create_time_json,parts_json,content) VALUES(?1,'other-source',NULL,?2,NULL,'user',NULL,NULL,'[]','legacy-owned')",
        [&conv, &candidate],
    )
    .unwrap();
    drop(seed);

    let mut child = observation("p", "s", "base", "child", "child-message");
    child.graph[0].parent_id = Some("parent".into());
    let mut combined = observation("p", "s", "base", "parent", "parent-message");
    combined.graph.push(child.graph[0].clone());
    let mut store = Store::open(&path).unwrap();
    assert_eq!(store.ingest_observation(&combined).unwrap(), conv);

    let db = Connection::open(&path).unwrap();
    let legacy: String = db
        .query_row(
            "SELECT content FROM messages WHERE id=?1",
            [&candidate],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(legacy, "legacy-owned");
    let parent_id: String = db
        .query_row(
            "SELECT message_id FROM graph_nodes WHERE conversation_id=?1 AND source_node_id='parent'",
            [&conv],
            |r| r.get(0),
        )
        .unwrap();
    assert_ne!(parent_id, candidate);
    let child_parent: String = db.query_row("SELECT parent_id FROM messages WHERE id=(SELECT message_id FROM graph_nodes WHERE conversation_id=?1 AND source_node_id='child')", [&conv], |r| r.get(0)).unwrap();
    assert_eq!(child_parent, parent_id);

    let normalized: Value = store.read(&conv).unwrap().unwrap();
    let before = normalized["nodes"].clone();
    assert_eq!(store.ingest_observation(&combined).unwrap(), conv);
    assert_eq!(store.read(&conv).unwrap().unwrap()["nodes"], before);
}

#[test]
fn exact_canonical_read_wins_alias_and_ambiguous_alias_is_none() {
    let mut store = Store::open(&db_path()).unwrap();
    let a = store
        .ingest_observation(&observation("p", "s", "a", "n", "m"))
        .unwrap();
    let b = store
        .ingest_observation(&observation("p", "s", "b", "n", "m"))
        .unwrap();
    // A legacy/source alias is only safe when it uniquely identifies a conversation.
    assert!(store.read_normalized("a").unwrap().is_none());
    assert!(store.read_normalized(&a).unwrap().is_some());
    assert!(store.read_normalized(&b).unwrap().is_some());
}

#[test]
fn tuple_delimiters_cannot_collide() {
    let mut store = Store::open(&db_path()).unwrap();
    let a = store
        .ingest_observation(&observation("a:b", "c", "d", "n", "m"))
        .unwrap();
    let b = store
        .ingest_observation(&observation("a", "b:c", "d", "n", "m"))
        .unwrap();
    assert_ne!(a, b);
}
