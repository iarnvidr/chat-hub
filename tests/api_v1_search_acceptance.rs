//! Additional acceptance cases, separate from the original five regressions.
//! Budget outcomes are observable here; pre-allocation enforcement additionally
//! requires source review of the exact F1 SQL/candidate collection path.
use chat_hub::{
    error::Error,
    store::{GraphNode, Observation, Store},
};
use serde_json::json;

fn node(id: &str, parent: Option<&str>) -> GraphNode {
    GraphNode {
        source_node_id: id.into(),
        source_message_id: Some(format!("provider-{id}")),
        parent_id: parent.map(str::to_owned),
        role: Some("assistant".into()),
        author: None,
        create_time: None,
        parts: vec![json!("routeprobe")],
    }
}
fn observation(source: &str, graph: Vec<GraphNode>) -> Observation {
    Observation {
        provider: "chatgpt".into(),
        account_scope: "provisional:default".into(),
        source_id: source.into(),
        title: "Synthetic search acceptance".into(),
        url: None,
        current_node: Some("z-valid".into()),
        graph,
        raw_payload: json!({"update_time":1700000000}),
        source_time: None,
        observed_ms: 1700000000000,
        parser: "f3-search-acceptance".into(),
        completeness: "complete".into(),
    }
}

#[test]
fn unroutable_cycle_hits_before_later_valid_hit_do_not_end_pagination() {
    let mut store = Store::in_memory().unwrap();
    // Equal text gives equal FTS ranks; node IDs put the two cyclic hits first.
    let id = store
        .ingest_observation(&observation(
            "cycles-first",
            vec![
                node("a-cycle", Some("b-cycle")),
                node("b-cycle", Some("a-cycle")),
                node("z-valid", None),
            ],
        ))
        .unwrap();
    let page = store
        .search_page_snapshot("routeprobe", 2, None, None)
        .unwrap();
    assert_eq!(
        page.hits.len(),
        1,
        "filtering unroutable hits must not consume the valid-result page budget"
    );
    assert_eq!(page.hits[0]["conversation"]["id"], id);
    assert_eq!(page.hits[0]["node_id"], "z-valid");
    assert_eq!(page.hits[0]["leaf_node_id"], "z-valid");
}

#[test]
fn oversized_candidate_title_and_ids_have_explicit_budget_failure() {
    for oversized_title in [true, false] {
        let mut store = Store::in_memory().unwrap();
        let mut observation = observation("candidate-budget", vec![node("z-valid", None)]);
        if oversized_title {
            observation.title = format!("routeprobe {}", "x".repeat(2 * 1024 * 1024));
        } else {
            let id = format!("z-{}", "x".repeat(2 * 1024 * 1024));
            observation.graph = vec![node(&id, None)];
            observation.current_node = Some(id);
        }
        store.ingest_observation(&observation).unwrap();
        let revision = store.library_revision().unwrap();
        assert!(
            matches!(
                store.search_page_snapshot("routeprobe", 2, None, None),
                Err(Error::ReadBudgetExceeded { .. })
            ),
            "oversized candidate metadata must fail explicitly"
        );
        assert_eq!(
            store.library_revision().unwrap(),
            revision,
            "read budget failure must not alter durable data"
        );
    }
}
