use chat_hub::{capture, importer, store::Store};
use serde_json::{json, Value};

fn graph() -> Value {
    serde_json::from_slice::<Value>(include_bytes!("fixtures/chatgpt-export/array.json")).unwrap()
        [0]
    .clone()
}

fn envelope(scope: &str, doc: &Value) -> Vec<u8> {
    serde_json::to_vec(
        &json!({"format":"chat-hub-history-v1", "account_scope":scope, "conversations":[doc]}),
    )
    .unwrap()
}

#[test]
fn fetched_graph_and_live_snapshot_share_account_identity_without_losing_branches() {
    let mut store = Store::in_memory().unwrap();
    let scope = format!("chatgpt:observed:{}", "a".repeat(64));
    let doc = graph();
    let bytes = envelope(&scope, &doc);
    assert_eq!(
        importer::import_chatgpt_history(&mut store, &bytes)
            .unwrap()
            .imported,
        1
    );
    assert_eq!(
        importer::import_chatgpt_history(&mut store, &bytes)
            .unwrap()
            .unchanged,
        1
    );
    let mut later = doc.clone();
    later["title"] = json!("Updated by a live snapshot");
    later["update_time"] = json!(1900000000);
    let observation =
        capture::normalize_chatgpt_document_in_scope(&later, "live-test", 1900000000000, &scope)
            .unwrap();
    store.ingest_observation(&observation).unwrap();
    let rows = store.library_snapshot().unwrap().conversations;
    assert_eq!(
        rows.len(),
        1,
        "fetch and capture must reconcile into the same authoritative conversation"
    );
    assert_eq!(rows[0]["account_scope"], scope);
    assert_eq!(rows[0]["title"], "Updated by a live snapshot");
    let id = rows[0]["id"].as_str().unwrap();
    let detail = store.detail_snapshot(id).unwrap().conversation.unwrap();
    assert_eq!(
        detail["nodes"].as_array().unwrap().len(),
        doc["mapping"].as_object().unwrap().len()
    );
    assert!(detail["messages"]
        .as_array()
        .unwrap()
        .iter()
        .any(|m| m["node_id"] == "left"));
    assert!(detail["messages"]
        .as_array()
        .unwrap()
        .iter()
        .any(|m| m["node_id"] == "right"));
}

#[test]
fn direct_accounts_are_isolated_and_generic_files_cannot_select_their_scope() {
    let mut store = Store::in_memory().unwrap();
    let doc = graph();
    for letter in ["a", "b"] {
        let scope = format!("chatgpt:observed:{}", letter.repeat(64));
        assert_eq!(
            importer::import_chatgpt_history(&mut store, &envelope(&scope, &doc))
                .unwrap()
                .imported,
            1
        );
    }
    let claimed = format!("chatgpt:observed:{}", "a".repeat(64));
    assert_eq!(
        importer::import_chatgpt(&mut store, &envelope(&claimed, &doc))
            .unwrap()
            .imported,
        1
    );
    let rows = store.library_snapshot().unwrap().conversations;
    assert_eq!(rows.len(), 3);
    assert!(rows
        .iter()
        .any(|c| c["account_scope"] == "provisional:default"));
    for scope in [
        "",
        "provisional:default",
        "chatgpt:observed:short",
        "chatgpt:oauth:../../other",
    ] {
        assert!(importer::import_chatgpt_history(&mut store, &envelope(scope, &doc)).is_err());
    }
    assert_eq!(store.library_snapshot().unwrap().conversations.len(), 3);
}
