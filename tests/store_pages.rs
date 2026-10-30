use chat_hub::store::{GraphNode, ListKey, Observation, Store};
use rusqlite::Connection;
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
    for (id, t) in [("a", 10), ("b", 10), ("c", 20)] {
        s.ingest_observation(&observation(id, id, t, vec![]))
            .unwrap();
    }
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
fn null_updated_at_rows_are_included_in_keyset_pages() {
    let path = std::env::temp_dir().join(format!("store-pages-null-{}.db", std::process::id()));
    let _ = std::fs::remove_file(&path);
    let mut s = Store::open(path.to_str().unwrap()).unwrap();
    for id in ["a", "b", "c"] {
        s.ingest_observation(&observation(id, id, 10, vec![]))
            .unwrap();
    }
    drop(s);
    Connection::open(&path)
        .unwrap()
        .execute(
            "UPDATE conversations SET updated_at_ms = NULL WHERE source_id = 'a'",
            [],
        )
        .unwrap();
    let s = Store::open(path.to_str().unwrap()).unwrap();
    let page = s.list_page_snapshot(1, None, None).unwrap();
    assert_eq!(page.items.len(), 1);
    let key = ListKey {
        updated_at_ms: page.items[0]["updated_at_ms"].as_i64(),
        id: page.items[0]["id"].as_str().unwrap().into(),
    };
    let next = s.list_page_snapshot(1, Some(&key), None).unwrap();
    assert_eq!(next.items.len(), 1);
    let third_key = ListKey {
        updated_at_ms: next.items[0]["updated_at_ms"].as_i64(),
        id: next.items[0]["id"].as_str().unwrap().into(),
    };
    let last = s.list_page_snapshot(1, Some(&third_key), None).unwrap();
    assert_eq!(last.items.len(), 1);
    assert!(s
        .list_page_snapshot(
            1,
            Some(&ListKey {
                updated_at_ms: last.items[0]["updated_at_ms"].as_i64(),
                id: last.items[0]["id"].as_str().unwrap().into()
            }),
            None
        )
        .unwrap()
        .items
        .is_empty());
    drop(s);
    std::fs::remove_file(path).unwrap();
}
#[test]
fn stale_revision_precedes_search_query() {
    let s = Store::in_memory().unwrap();
    assert!(s
        .search_page_snapshot("[invalid FTS ???", 10, None, Some(-1))
        .unwrap()
        .hits
        .is_empty());
}
#[test]
fn detail_rejects_large_parts_without_truncating_database() {
    let mut s = Store::in_memory().unwrap();
    let id = s
        .ingest_observation(&observation(
            "large",
            "big",
            1,
            vec![GraphNode {
                parts: vec![json!({"opaque": "x".repeat(2 * 1024 * 1024)})],
                ..node("n", None, "small searchable text")
            }],
        ))
        .unwrap();
    assert!(matches!(
        s.detail_snapshot(&id),
        Err(chat_hub::error::Error::ReadBudgetExceeded { .. })
    ));
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
#[test]
fn huge_parts_list_summary_search_preflight_and_null_cursor_walk() {
    let mut s = Store::in_memory().unwrap();
    let huge = s
        .ingest_observation(&observation(
            "huge",
            "needle",
            1,
            vec![GraphNode {
                parts: vec![
                    json!("needle"),
                    json!({"opaque": "x".repeat(2 * 1024 * 1024)}),
                ],
                ..node("n", None, "needle")
            }],
        ))
        .unwrap();
    let page = s.list_page_snapshot(1, None, None).unwrap();
    let c = &page.items[0];
    assert_eq!(c.as_object().unwrap().len(), 6);
    assert_eq!(c["id"], huge);
    assert!(matches!(
        s.search_page_snapshot("needle", 10, None, None),
        Err(chat_hub::error::Error::ReadBudgetExceeded { .. })
    ));
}

#[test]
fn search_scan_candidate_cap_returns_typed_error() {
    let mut s = Store::in_memory().unwrap();
    let graph = (0..1025)
        .map(|i| {
            let id = format!("n{i:04}");
            GraphNode {
                source_node_id: id.clone(),
                source_message_id: Some(id.clone()),
                parent_id: Some(id),
                role: Some("assistant".into()),
                author: None,
                create_time: None,
                parts: vec![json!("scanprobe")],
            }
        })
        .collect();
    let mut obs = observation("scan-cap", "no token in title", 1, graph);
    obs.current_node = Some("n1024".into());
    s.ingest_observation(&obs).unwrap();
    let result = s.search_page_snapshot("scanprobe", 1, None, None);
    assert!(
        matches!(
            result,
            Err(chat_hub::error::Error::ReadBudgetExceeded {
                kind: "search_scan_candidates",
                limit: 1024,
            })
        ),
        "unexpected scan-cap outcome: {result:?}"
    );
}

#[test]
fn search_refills_past_cyclic_graph_candidates() {
    let mut s = Store::in_memory().unwrap();
    let cyc = vec![
        GraphNode {
            parent_id: Some("b-cycle".into()),
            ..node("a-cycle", None, "routeprobe")
        },
        GraphNode {
            parent_id: Some("a-cycle".into()),
            ..node("b-cycle", None, "routeprobe")
        },
    ];
    s.ingest_observation(&observation("cycle", "cycle", 1, cyc))
        .unwrap();
    let valid = vec![GraphNode {
        source_node_id: "z-valid".into(),
        source_message_id: Some("z-valid".into()),
        parent_id: None,
        role: Some("assistant".into()),
        author: None,
        create_time: None,
        parts: vec![json!("routeprobe")],
    }];
    s.ingest_observation(&observation("valid", "valid", 1, valid))
        .unwrap();
    let page = s.search_page_snapshot("routeprobe", 2, None, None).unwrap();
    assert!(page.hits.iter().any(|h| h["node_id"] == "z-valid"));
}
