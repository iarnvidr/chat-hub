use chat_hub::store::{GraphNode, Observation, Store};
use rusqlite::Connection;
use serde_json::{json, Value};

fn obs(source: &str, text: &str) -> Observation {
    Observation {
        provider: "test".into(),
        account_scope: "account".into(),
        source_id: source.into(),
        title: "title".into(),
        url: None,
        current_node: None,
        graph: vec![GraphNode {
            source_node_id: "n".into(),
            source_message_id: None,
            parent_id: None,
            role: Some("user".into()),
            author: None,
            create_time: None,
            parts: vec![Value::String(text.into())],
        }],
        raw_payload: json!({}),
        source_time: None,
        observed_ms: 1,
        parser: "test".into(),
        completeness: "complete".into(),
    }
}
fn temp(name: &str) -> String {
    std::env::temp_dir()
        .join(format!(
            "chat-hub-{name}-{}-{}.db",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
        .to_string_lossy()
        .into_owned()
}
fn fk_ok(path: &str) {
    let c = Connection::open(path).unwrap();
    let n: i64 = c
        .query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(n, 0);
}

#[test]
fn empty_file_opens_at_v3() {
    let p = temp("empty");
    let s = Store::open(&p).unwrap();
    assert_eq!(s.schema_version().unwrap(), 3);
    drop(s);
    fk_ok(&p);
    std::fs::remove_file(p).unwrap();
}
#[test]
fn historical_v1_and_v2_fixtures_migrate_and_reopen_without_changes() {
    for version in [1, 2] {
        let p = temp(&format!("v{version}"));
        let c = Connection::open(&p).unwrap();
        c.execute_batch(if version == 1 {
            include_str!("fixtures/store/v1.sql")
        } else {
            include_str!("fixtures/store/v2.sql")
        })
        .unwrap();
        drop(c);
        let s = Store::open(&p).unwrap();
        assert_eq!(s.schema_version().unwrap(), 3);
        assert!(!s.search(&format!("v{version}"), 10).unwrap().is_empty());
        drop(s);
        fk_ok(&p);
        let c = Connection::open(&p).unwrap();
        let before: (String,String,String)=c.query_row("SELECT c.id,g.source_node_id,g.content FROM conversations c JOIN graph_nodes g ON g.conversation_id=c.id",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).unwrap();
        assert_eq!(before.0, format!("legacy-v{version}"));
        assert_eq!(before.1, "legacy-message");
        drop(c);
        let s = Store::open(&p).unwrap();
        assert_eq!(s.schema_version().unwrap(), 3);
        drop(s);
        let c = Connection::open(&p).unwrap();
        let after: (String,String,String)=c.query_row("SELECT c.id,g.source_node_id,g.content FROM conversations c JOIN graph_nodes g ON g.conversation_id=c.id",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).unwrap();
        assert_eq!(after, before);
        drop(c);
        fk_ok(&p);
        std::fs::remove_file(p).unwrap();
    }
}
#[test]
fn v3_failure_rolls_back_alter_and_retry_succeeds() {
    let p = temp("rollback");
    let c = Connection::open(&p).unwrap();
    c.execute_batch(include_str!("fixtures/store/v1.sql"))
        .unwrap();
    c.execute_batch("CREATE TABLE graph_nodes(injected INTEGER)")
        .unwrap();
    drop(c);
    assert!(Store::open(&p).is_err());
    let c = Connection::open(&p).unwrap();
    let version: i64 = c
        .query_row("SELECT max(version) FROM schema_migrations", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(version, 2);
    let cols: Vec<String> = {
        let mut q = c.prepare("PRAGMA table_info(conversations)").unwrap();
        q.query_map([], |r| r.get(1))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    };
    assert!(!cols.contains(&"account_scope".into()));
    assert_eq!(
        c.query_row::<String, _, _>(
            "SELECT content FROM messages WHERE id='legacy-message'",
            [],
            |r| r.get(0)
        )
        .unwrap(),
        "v1 searchable"
    );
    c.execute_batch("DROP TABLE graph_nodes").unwrap();
    drop(c);
    let s = Store::open(&p).unwrap();
    assert_eq!(s.schema_version().unwrap(), 3);
    assert_eq!(s.search("searchable", 10).unwrap().len(), 1);
    drop(s);
    fk_ok(&p);
    std::fs::remove_file(p).unwrap();
}
#[test]
fn migrated_schema_is_idempotent_and_fts_tracks_insert_update_delete_rebuild() {
    let p = temp("fts");
    let mut s = Store::open(&p).unwrap();
    assert_eq!(s.schema_version().unwrap(), 3);
    let id = s.ingest_observation(&obs("a", "alphaunique")).unwrap();
    assert_eq!(s.search("alphaunique", 10).unwrap(), vec![id.clone()]);
    s.ingest_observation(&obs("a", "betaunique")).unwrap();
    assert!(s.search("alphaunique", 10).unwrap().is_empty());
    assert_eq!(s.search("betaunique", 10).unwrap(), vec![id.clone()]);
    s.ingest_observation(&obs("b", "rebuildunique")).unwrap();
    s.rebuild_search_index().unwrap();
    assert_eq!(s.search("rebuildunique", 10).unwrap().len(), 1);
    drop(s);
    let c = Connection::open(&p).unwrap();
    c.execute("DELETE FROM graph_nodes WHERE conversation_id=?1", [&id])
        .unwrap();
    drop(c);
    let s = Store::open(&p).unwrap();
    assert!(s.search("betaunique", 10).unwrap().is_empty());
    drop(s);
    let c = Connection::open(&p).unwrap();
    c.execute("DELETE FROM conversations WHERE id=?1", [&id])
        .unwrap();
    drop(c);
    let s = Store::open(&p).unwrap();
    assert_eq!(s.schema_version().unwrap(), 3);
    assert!(s.search("betaunique", 10).unwrap().is_empty());
    drop(s);
    fk_ok(&p);
    std::fs::remove_file(p).unwrap();
}
