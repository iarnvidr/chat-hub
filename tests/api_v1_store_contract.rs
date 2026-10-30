//! Integration regressions for the EXACT bounded Store methods F3 consumes.
//! No SQL, alternate schema, full-snapshot substitution, or canned DTO backend.
use chat_hub::{
    error::Error,
    server::api_v1::{SearchHit, Summary},
    store::{GraphNode, ListKey, Observation, Store},
};
use serde_json::{json, Value};

fn observation(source: &str, text: &str) -> Observation {
    let node = |id: &str, parent: Option<&str>, role: Option<&str>, parts: Vec<Value>| GraphNode {
        source_node_id: id.into(),
        source_message_id: role.map(|_| format!("provider-{id}")),
        parent_id: parent.map(str::to_owned),
        role: role.map(str::to_owned),
        author: role.map(|role| json!({"role":role})),
        create_time: None,
        parts,
    };
    Observation {
        provider: "chatgpt".into(),
        account_scope: "provisional:default".into(),
        source_id: source.into(),
        title: "Synthetic bounded summary".into(),
        url: None,
        current_node: Some("a".into()),
        graph: vec![
            node("root", None, None, vec![]),
            node(
                "user",
                Some("root"),
                Some("user"),
                vec![json!("amber telescope")],
            ),
            node("a", Some("user"), Some("assistant"), vec![json!(text)]),
        ],
        raw_payload: json!({"update_time":1700000000}),
        source_time: None,
        observed_ms: 1700000000000,
        parser: "f3-bounded-contract".into(),
        completeness: "complete".into(),
    }
}

#[test]
fn bounded_list_returns_summaries_even_when_detail_exceeds_budget() {
    let mut store = Store::in_memory().unwrap();
    let id = store
        .ingest_observation(&observation(
            "large-list",
            "x".repeat(2 * 1024 * 1024).as_str(),
        ))
        .unwrap();
    assert!(matches!(
        store.detail_snapshot(&id),
        Err(Error::ReadBudgetExceeded { .. })
    ));
    let page = store.list_page_snapshot(2, None, None).unwrap();
    assert_eq!(page.items.len(), 1);
    assert_eq!(
        page.items[0].as_object().unwrap().len(),
        7,
        "bounded list must select summary columns, never a full graph DTO"
    );
    assert!(page.items[0].get("nodes").is_none() && page.items[0].get("messages").is_none());
    let summary: Summary = serde_json::from_value(page.items[0].clone()).unwrap();
    assert_eq!(summary.id, id);
    assert_eq!(summary.account_scope, "provisional:default");
}

#[test]
fn bounded_search_returns_complete_same_snapshot_summary_and_message_anchor() {
    let mut store = Store::in_memory().unwrap();
    let id = store
        .ingest_observation(&observation("search-summary", "amber telescope reply"))
        .unwrap();
    let page = store
        .search_page_snapshot("amber telescope", 101, None, None)
        .unwrap();
    assert_eq!(page.library_revision, store.library_revision().unwrap());
    assert_eq!(page.hits.len(), 2);
    for hit in page.hits {
        let hit:SearchHit=serde_json::from_value(hit).expect("same-transaction complete summary required; id-only lookup cannot be repaired by a later read");
        assert_eq!(hit.conversation.id, id);
        assert_eq!(hit.conversation.provider, "chatgpt");
        assert!(hit.node_id.is_some() && hit.message_id.is_some() && hit.leaf_node_id.is_some());
        assert!(hit.score.is_finite());
        assert!(hit.selected_branch);
    }
}

#[test]
fn bounded_unknown_time_keyset_does_not_restart_known_time_rows() {
    let mut store = Store::in_memory().unwrap();
    store
        .ingest_observation(&observation("known-time", "reply"))
        .unwrap();
    let after = ListKey {
        updated_at_ms: None,
        id: "unknown-time-anchor".into(),
    };
    let page = store.list_page_snapshot(101, Some(&after), None).unwrap();
    assert!(
        page.items.is_empty(),
        "after an unknown-time key, known-time rows must never be returned"
    );
}

#[test]
fn bounded_search_preflights_large_source_graph_before_materializing_dto() {
    let mut store = Store::in_memory().unwrap();
    let text = format!("needle {}", "x".repeat(2 * 1024 * 1024));
    store
        .ingest_observation(&observation("large-search", &text))
        .unwrap();
    assert!(
        matches!(
            store.search_page_snapshot("needle", 2, None, None),
            Err(Error::ReadBudgetExceeded { .. })
        ),
        "bounded search must reject oversized graph source before loading its full payload"
    );
}

#[test]
fn bounded_detail_ancestry_is_root_to_selected_without_full_snapshot_substitution() {
    let mut store = Store::in_memory().unwrap();
    let id = store
        .ingest_observation(&observation("detail-ancestry", "reply"))
        .unwrap();
    let detail = store.detail_snapshot(&id).unwrap().conversation.unwrap();
    assert_eq!(detail["thread_node_ids"], json!(["root", "user", "a"]));
    assert_eq!(detail["thread_complete"], true);
}
