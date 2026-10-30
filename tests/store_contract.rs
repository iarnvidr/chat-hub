use chat_hub::store::{GraphNode, Observation, Store};
use serde_json::{json, Value};

fn replay(name: &str) -> (Observation, Value) {
    let fixture: Value = serde_json::from_str(match name {
        "selected" => include_str!("fixtures/api-v1/conversation-selected.json"),
        "alternate" => include_str!("fixtures/api-v1/conversation-alternate.json"),
        "fallback" => include_str!("fixtures/api-v1/conversation-fallback.json"),
        _ => unreachable!(),
    })
    .unwrap();
    let c = &fixture["conversation"];
    let mut nodes = Vec::new();
    for node in fixture["nodes"].as_array().unwrap() {
        let message = fixture["messages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["node_id"] == node["id"]);
        nodes.push(GraphNode {
            source_node_id: node["source_id"].as_str().unwrap().to_owned(),
            source_message_id: message
                .map(|m| m["source_id"].clone())
                .filter(|v| !v.is_null())
                .and_then(|v| v.as_str().map(str::to_owned)),
            parent_id: node["parent_id"].as_str().map(str::to_owned),
            role: message.and_then(|m| m["role"].as_str()).map(str::to_owned),
            author: message
                .map(|m| m["author"].clone())
                .filter(|v| !v.is_null()),
            create_time: message
                .map(|m| {
                    if !m["create_time"].is_null() {
                        m["create_time"].clone()
                    } else if let Some(ms) = m["created_at_ms"].as_i64() {
                        json!(ms as f64 / 1000.0)
                    } else {
                        Value::Null
                    }
                })
                .filter(|v| !v.is_null()),
            parts: message
                .map(|m| {
                    m["parts"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|p| p["raw"].clone())
                        .collect()
                })
                .unwrap_or_default(),
        });
    }
    let observation = Observation {
        provider: c["provider"].as_str().unwrap().into(),
        account_scope: c["account_scope"].as_str().unwrap().into(),
        source_id: c["source_id"].as_str().unwrap().into(),
        title: c["title"].as_str().unwrap().into(),
        url: None,
        current_node: fixture["selection"]["source_current_node_id"]
            .as_str()
            .map(str::to_owned),
        graph: nodes,
        raw_payload: json!({"update_time": c["updated_at_ms"].as_f64().unwrap()/1000.0}),
        source_time: None,
        observed_ms: c["updated_at_ms"].as_i64().unwrap(),
        parser: "api-v1-fixture".into(),
        completeness: "complete".into(),
    };
    (observation, fixture)
}

#[test]
fn api_v1_fixtures_replay_through_persisted_store() {
    for scenario in ["selected", "alternate", "fallback"] {
        let mut store = Store::in_memory().unwrap();
        let (observation, fixture) = replay(scenario);
        let id = store.ingest_observation(&observation).unwrap();
        let actual = store.read_normalized(&id).unwrap().unwrap();
        let bounded = store.detail_snapshot(&id).unwrap().conversation.unwrap();
        assert_eq!(bounded, actual);
        let c = &fixture["conversation"];
        assert_eq!(actual["id"], id);
        assert_eq!(actual["provider"], c["provider"]);
        assert_eq!(actual["account_scope"], c["account_scope"]);
        assert!(!actual["account_scope"].is_null());
        assert_eq!(actual["source_id"], c["source_id"]);
        let nodes = actual["nodes"].as_array().unwrap();
        assert_eq!(nodes.len(), fixture["nodes"].as_array().unwrap().len());
        for source_node in fixture["nodes"].as_array().unwrap() {
            let node = nodes
                .iter()
                .find(|n| n["source_id"] == source_node["source_id"])
                .unwrap();
            assert_eq!(node["parent_id"], source_node["parent_id"]);
            assert!(!node["source_id"].is_null());
            if !source_node["message_id"].is_null() {
                let source_message = fixture["messages"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|m| m["id"] == source_node["message_id"])
                    .unwrap();
                let message = actual["messages"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|m| m["node_id"] == node["id"])
                    .unwrap();
                assert_eq!(message["source_id"], source_message["source_id"]);
                assert_eq!(message["role"], source_message["role"]);
                assert_eq!(message["author"], source_message["author"]);
                assert_eq!(message["created_at_ms"], source_message["created_at_ms"]);
                assert_eq!(
                    message["parts"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|p| p["raw"].clone())
                        .collect::<Vec<_>>(),
                    source_message["parts"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|p| p["raw"].clone())
                        .collect::<Vec<_>>()
                );
                assert_ne!(message["id"], source_message["id"]);
            } else {
                assert!(node["message_id"].is_null());
            }
        }
        // Fixture leaf overrides are view-only; persistence stores the source pointer,
        // then normalized selection follows that pointer (or fallback if invalid).
        let expected_selected = if scenario == "alternate" {
            "n-a"
        } else {
            fixture["selection"]["selected_node_id"].as_str().unwrap()
        };
        assert_eq!(actual["selected_node_id"], expected_selected);
        let expected_path: Vec<Value> = if scenario == "alternate" {
            vec![json!("n-root"), json!("n-user"), json!("n-a")]
        } else {
            fixture["thread_node_ids"]
                .as_array()
                .unwrap()
                .iter()
                .map(|id| json!(id.as_str().unwrap()))
                .collect()
        };
        let expected_path: Vec<Value> = expected_path
            .into_iter()
            .map(|id| {
                json!(
                    fixture["nodes"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .find(|n| n["id"] == id)
                        .unwrap()["source_id"]
                )
            })
            .collect();
        assert_eq!(actual["thread_node_ids"], json!(expected_path));
        // Completeness describes the selected thread, not whether the raw source
        // pointer itself names a known node (fallback can select another leaf).
        let expected_complete = if scenario == "alternate" {
            fixture["selection"]["thread_complete"].as_bool().unwrap()
        } else {
            let expected_leaf = expected_path.last().unwrap().as_str().unwrap();
            let mut current = Some(expected_leaf);
            let mut visited = Vec::new();
            let mut complete = false;
            while let Some(node_id) = current {
                if visited.contains(&node_id) {
                    break;
                }
                visited.push(node_id);
                let Some(node) = fixture["nodes"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|node| node["source_id"] == node_id)
                else {
                    break;
                };
                match node["parent_id"].as_str() {
                    Some(parent) => current = Some(parent),
                    None => {
                        complete = true;
                        break;
                    }
                }
            }
            complete
        };
        if scenario == "fallback" {
            assert_eq!(fixture["selection"]["reason"], "fallback_leaf");
            assert_ne!(
                fixture["selection"]["source_current_node_id"],
                fixture["selection"]["selected_node_id"]
            );
        }
        assert_eq!(actual["thread_complete"], expected_complete);
    }
}
