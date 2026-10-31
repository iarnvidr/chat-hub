//! HTTP routes backed by the actual bounded Store and shared importer.
use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
    Router,
};
use chat_hub::{
    server,
    store::{GraphNode, Observation, Store},
};
use serde_json::{json, Value};
use tower::ServiceExt;

fn app() -> Router {
    server::router(Store::in_memory().unwrap(), "secret".into())
}

fn document(id: &str, time: i64) -> Value {
    let mut doc: Value =
        serde_json::from_slice::<Value>(include_bytes!("fixtures/chatgpt-export/array.json"))
            .unwrap()[0]
            .clone();
    doc["id"] = json!(id);
    doc["update_time"] = json!(time);
    doc["mapping"]["left"]["message"]["create_time"] = Value::Null;
    doc["mapping"]["right"]["message"]["create_time"] = json!(1700000002.1259);
    doc["mapping"]["left"]["message"]["content"]["parts"][0] =
        json!("bodyonlyneedle alternate answer");
    doc["mapping"]["right"]["message"]["content"]["parts"][0] =
        json!("bodyonlyneedle selected answer\n\n```rust\nlet n = 1;\n```");
    doc
}

fn source_observation(provider: &str, id: &str, title: &str, time: i64) -> Observation {
    let node_id = format!("{id}-message");
    Observation {
        provider: provider.into(),
        account_scope: "paseo:test-owner".into(),
        source_id: id.into(),
        title: title.into(),
        url: None,
        current_node: Some(node_id.clone()),
        graph: vec![GraphNode {
            source_node_id: node_id.clone(),
            source_message_id: Some(node_id),
            parent_id: None,
            role: Some("user".into()),
            author: None,
            create_time: None,
            parts: vec![json!("chatfilterneedle shared")],
        }],
        raw_payload: Value::Null,
        source_time: None,
        observed_ms: time,
        parser: "test".into(),
        completeness: "complete".into(),
    }
}

async fn request(app: &Router, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
    let mut req = Request::builder()
        .uri(uri)
        .header("authorization", "Bearer secret");
    let body = match body {
        Some(value) => {
            req = req
                .method("POST")
                .header("content-type", "application/json");
            Body::from(serde_json::to_vec(&value).unwrap())
        }
        None => Body::empty(),
    };
    let response = app.clone().oneshot(req.body(body).unwrap()).await.unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 9 * 1024 * 1024)
        .await
        .unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

async fn import(app: &Router, docs: Value) -> Value {
    let (status, receipt) = request(app, "/api/v1/imports/chatgpt", Some(docs)).await;
    assert_eq!(status, StatusCode::OK, "{receipt}");
    assert_eq!(receipt["failed"], 0, "{receipt}");
    receipt
}

#[tokio::test]
async fn large_local_transcript_detail_and_search_preserve_exact_text() {
    let mut store = Store::in_memory().unwrap();
    let text = "archivebodyneedle café 🧭 \"quotation\"\n".repeat(25_000);
    let mut o = source_observation("paseo-chatgpt", "large-cache", "Large saved chat", 1);
    o.graph[0].role = Some("archive".into());
    o.graph[0].parts = vec![json!(text)];
    o.completeness = "cached_transcript".into();
    let id = store.ingest_observation(&o).unwrap();
    let app = server::router(store, "secret".into());
    let (status, detail) = request(&app, &format!("/api/v1/conversations/{id}"), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(detail["messages"][0]["role"], "archive");
    assert_eq!(detail["messages"][0]["content"], text);
    assert_eq!(detail["messages"][0]["parts"][0]["text"], text);
    assert_eq!(detail["messages"][0]["parts"][0]["raw"], text);
    let (status, search) = request(&app, "/api/v1/chat/search?q=archivebodyneedle", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(search["items"].as_array().unwrap().len(), 1);
    assert_eq!(search["items"][0]["node_id"], o.current_node.unwrap());
}

#[tokio::test]
async fn import_list_search_anchor_and_view_only_branch_use_real_store() {
    let app = app();
    assert_eq!(
        import(&app, json!([document("one", 1700000100)])).await["imported"],
        1
    );
    let (status, list) = request(&app, "/api/v1/conversations", None).await;
    assert_eq!(status, StatusCode::OK);
    let id = list["items"][0]["id"].as_str().unwrap();
    assert_ne!(id, "one");
    assert_eq!(list["items"][0].as_object().unwrap().len(), 6);
    assert_eq!(list["items"][0]["project"], Value::Null);
    let path = format!("/api/v1/conversations/{id}");
    let (status, detail) = request(&app, &path, None).await;
    assert_eq!(status, StatusCode::OK, "{detail}");
    assert_eq!(detail["selection"]["selected_node_id"], "right");
    assert_eq!(detail["thread_node_ids"], json!(["root", "right"]));
    assert_eq!(detail["nodes"].as_array().unwrap().len(), 3);
    let messages = detail["messages"].as_array().unwrap();
    let right = messages.iter().find(|m| m["node_id"] == "right").unwrap();
    let left = messages.iter().find(|m| m["node_id"] == "left").unwrap();
    assert_eq!(right["created_at_ms"], 1700000002125_i64);
    assert_eq!(left["created_at_ms"], Value::Null);
    assert_eq!(right["parts"][1]["kind"], "unknown");
    assert_eq!(right["parts"][1]["raw"]["content_type"], "future_part");
    let (status, search) = request(&app, "/api/v1/search?q=bodyonlyneedle", None).await;
    assert_eq!(status, StatusCode::OK, "{search}");
    assert_eq!(search["items"].as_array().unwrap().len(), 2);
    let hit = search["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|hit| hit["node_id"] == "left")
        .unwrap();
    assert_eq!(hit["message_id"], left["id"]);
    assert_eq!(hit["leaf_node_id"], "left");
    assert_eq!(hit["selected_branch"], false);
    assert!(hit["snippet"].as_str().unwrap().contains("bodyonlyneedle"));
    assert!(!hit["snippet"].as_str().unwrap().contains("<mark>"));
    let (status, alternate) = request(&app, &format!("{path}?leaf_node_id=left"), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(alternate["selection"]["reason"], "leaf_override");
    assert_eq!(alternate["thread_node_ids"], json!(["root", "left"]));
    for key in ["nodes", "messages", "library_revision"] {
        assert_eq!(alternate[key], detail[key]);
    }
    assert_eq!(request(&app, &path, None).await.1, detail);
}

#[tokio::test]
async fn mutation_stales_list_and_search_cursors_but_noop_does_not() {
    let app = app();
    let docs = json!([document("one", 100), document("two", 200)]);
    import(&app, docs.clone()).await;
    let mut pages = Vec::new();
    for path in [
        "/api/v1/conversations?limit=1",
        "/api/v1/search?q=bodyonlyneedle&limit=1",
    ] {
        let (status, page) = request(&app, path, None).await;
        assert_eq!(status, StatusCode::OK);
        let cursor = page["next_cursor"].as_str().unwrap();
        pages.push(format!("{path}&cursor={cursor}"));
    }
    assert_eq!(import(&app, docs).await["unchanged"], 2);
    for path in &pages {
        assert_eq!(request(&app, path, None).await.0, StatusCode::OK);
    }
    assert_eq!(
        import(&app, json!([document("three", 300)])).await["imported"],
        1
    );
    for path in &pages {
        let (status, error) = request(&app, path, None).await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(error["error"]["code"], "stale_cursor");
    }
}

#[tokio::test]
async fn empty_unknown_and_oversized_detail_have_explicit_responses() {
    let app = app();
    for path in ["/api/v1/conversations", "/api/v1/search?q=absent"] {
        let (status, page) = request(&app, path, None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(page["items"], json!([]));
        assert_eq!(page["next_cursor"], Value::Null);
    }
    assert_eq!(
        request(&app, "/api/v1/conversations/unknown", None).await.0,
        StatusCode::NOT_FOUND
    );
    let mut doc = document("huge", 100);
    doc["mapping"]["right"]["message"]["content"]["parts"][0] = json!("x".repeat(9 * 1024 * 1024));
    import(&app, json!([doc])).await;
    let (status, list) = request(&app, "/api/v1/conversations", None).await;
    assert_eq!(status, StatusCode::OK);
    let id = list["items"][0]["id"].as_str().unwrap();
    let (status, error) = request(&app, &format!("/api/v1/conversations/{id}"), None).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(error["error"]["code"], "response_too_large");
    assert!(error.get("nodes").is_none());
    assert!(error.get("messages").is_none());
}

#[tokio::test]
async fn chat_routes_filter_server_side_and_bind_their_cursors() {
    let mut store = Store::in_memory().unwrap();
    for (provider, id, title, time) in [
        ("chatgpt", "chat-old", "ChatGPT older", 10),
        ("paseo-chatgpt", "chat-new", "Paseo ChatGPT newer", 30),
        ("paseo-codex_local", "codex-mid", "Codex in between", 20),
    ] {
        store
            .ingest_observation(&source_observation(provider, id, title, time))
            .unwrap();
    }
    let app = server::router(store, "secret".into());

    let (status, chat_first) = request(&app, "/api/v1/chat/conversations?limit=1", None).await;
    assert_eq!(status, StatusCode::OK, "{chat_first}");
    let chat_cursor = chat_first["next_cursor"].as_str().unwrap();
    assert!(matches!(
        chat_first["items"][0]["provider"].as_str(),
        Some("chatgpt" | "paseo-chatgpt")
    ));
    let (status, chat_second) = request(
        &app,
        &format!("/api/v1/chat/conversations?limit=1&cursor={chat_cursor}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{chat_second}");
    assert_eq!(chat_second["items"].as_array().unwrap().len(), 1);
    assert!(matches!(
        chat_second["items"][0]["provider"].as_str(),
        Some("chatgpt" | "paseo-chatgpt")
    ));
    assert!(chat_second["next_cursor"].is_null());

    let (status, all_first) = request(&app, "/api/v1/conversations?limit=1", None).await;
    assert_eq!(status, StatusCode::OK);
    let all_cursor = all_first["next_cursor"].as_str().unwrap();
    let (status, all_second) = request(
        &app,
        &format!("/api/v1/conversations?limit=1&cursor={all_cursor}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{all_second}");
    assert!(all_second["items"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| item["provider"] == "paseo-codex_local"));
    assert_eq!(
        request(
            &app,
            &format!("/api/v1/conversations?cursor={chat_cursor}"),
            None,
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        request(
            &app,
            &format!("/api/v1/chat/conversations?cursor={all_cursor}"),
            None,
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );

    let (status, chat_search_first) =
        request(&app, "/api/v1/chat/search?q=chatfilterneedle&limit=1", None).await;
    assert_eq!(status, StatusCode::OK, "{chat_search_first}");
    let chat_search_cursor = chat_search_first["next_cursor"].as_str().unwrap();
    assert!(matches!(
        chat_search_first["items"][0]["conversation"]["provider"].as_str(),
        Some("chatgpt" | "paseo-chatgpt")
    ));
    let (status, chat_search_second) = request(
        &app,
        &format!("/api/v1/chat/search?q=chatfilterneedle&limit=1&cursor={chat_search_cursor}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{chat_search_second}");
    assert_eq!(chat_search_second["items"].as_array().unwrap().len(), 1);
    assert!(matches!(
        chat_search_second["items"][0]["conversation"]["provider"].as_str(),
        Some("chatgpt" | "paseo-chatgpt")
    ));
    assert!(chat_search_second["next_cursor"].is_null());

    let (status, all_search) =
        request(&app, "/api/v1/search?q=chatfilterneedle&limit=1", None).await;
    assert_eq!(status, StatusCode::OK, "{all_search}");
    let all_search_cursor = all_search["next_cursor"].as_str().unwrap();
    let (status, all_search_second) = request(
        &app,
        &format!("/api/v1/search?q=chatfilterneedle&limit=1&cursor={all_search_cursor}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{all_search_second}");
    let (status, all_search_third) = request(
        &app,
        &format!(
            "/api/v1/search?q=chatfilterneedle&limit=1&cursor={}",
            all_search_second["next_cursor"].as_str().unwrap()
        ),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{all_search_third}");
    assert!(matches!(
        all_search_third["items"][0]["conversation"]["provider"].as_str(),
        Some("chatgpt" | "paseo-chatgpt" | "paseo-codex_local")
    ));
    assert_eq!(
        request(
            &app,
            &format!("/api/v1/search?q=chatfilterneedle&cursor={chat_search_cursor}"),
            None,
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        request(
            &app,
            &format!("/api/v1/chat/search?q=chatfilterneedle&cursor={all_search_cursor}"),
            None,
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
}
