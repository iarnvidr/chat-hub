use axum::{body::Body, http::Request};
use chat_hub::{
    server,
    store::{GraphNode, Observation, Store},
};
use serde_json::{json, Value};
use tower::ServiceExt;

#[tokio::test]
async fn state_route_rejects_bad_receipts_and_filters_active_archived_lists() {
    let scope = format!("chatgpt:observed:{}", "c".repeat(64));
    let mut store = Store::in_memory().unwrap();
    let conversation_id = store
        .ingest_observation(&Observation {
            provider: "chatgpt".into(),
            account_scope: scope.clone(),
            source_id: "chat-state-uuid".into(),
            title: "State route test".into(),
            url: None,
            current_node: None,
            graph: vec![],
            raw_payload: Value::Null,
            source_time: None,
            observed_ms: 1,
            parser: "test".into(),
            completeness: "complete".into(),
        })
        .unwrap();
    let app = server::router(store, "state-token".into());
    let receipt = json!({
        "verified": true,
        "conversation_id": conversation_id.clone(),
        "source_conversation_id": "chat-state-uuid",
        "scope": scope.clone(),
        "target_provider": "chatgpt",
        "target_scope": format!("chatgpt:observed:{}", "c".repeat(64)),
        "archived": true,
        "pinned": true
    });
    let uri = format!("/api/v1/conversations/{conversation_id}/state");
    let send = |body: Value| {
        app.clone().oneshot(
            Request::builder()
                .method("PATCH")
                .uri(&uri)
                .header("authorization", "Bearer state-token")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
    };

    let mut invalid_receipt = receipt.clone();
    invalid_receipt["source_conversation_id"] = json!("different-chat");
    let rejected = send(json!({
        "archived": true,
        "pinned": true,
        "receipt": invalid_receipt
    }))
    .await
    .unwrap();
    assert_eq!(rejected.status(), axum::http::StatusCode::BAD_REQUEST);

    let saved = send(json!({
        "archived": true,
        "pinned": true,
        "receipt": receipt
    }))
    .await
    .unwrap();
    assert_eq!(saved.status(), axum::http::StatusCode::OK);
    let saved_body: Value = serde_json::from_slice(
        &axum::body::to_bytes(saved.into_body(), 1024 * 1024)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(saved_body["verified"], true);
    assert_eq!(saved_body["pinned"], true);
    assert_eq!(saved_body["archived"], true);

    let identity = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(&uri)
                .header("authorization", "Bearer state-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(identity.status(), axum::http::StatusCode::OK);
    let identity_body: Value = serde_json::from_slice(
        &axum::body::to_bytes(identity.into_body(), 64 * 1024)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(identity_body["conversation_id"], conversation_id);
    assert_eq!(identity_body["provider"], "chatgpt");
    assert_eq!(identity_body["source_id"], "chat-state-uuid");
    assert_eq!(identity_body["pinned"], true);
    assert_eq!(identity_body["archived"], true);
    assert!(identity_body.get("messages").is_none());

    let active = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/conversations?project_key=__none__&archive_state=active&limit=10")
                .header("authorization", "Bearer state-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(active.status(), axum::http::StatusCode::OK);
    let active_body: Value = serde_json::from_slice(
        &axum::body::to_bytes(active.into_body(), 1024 * 1024)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(active_body["items"].as_array().unwrap().len(), 0);

    let archived = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/conversations?project_key=__none__&archive_state=archived&limit=10")
                .header("authorization", "Bearer state-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(archived.status(), axum::http::StatusCode::OK);
    let archived_body: Value = serde_json::from_slice(
        &axum::body::to_bytes(archived.into_body(), 1024 * 1024)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(archived_body["items"].as_array().unwrap().len(), 1);
    assert_eq!(archived_body["items"][0]["pinned"], true);
    assert_eq!(archived_body["items"][0]["archived"], true);
}

#[tokio::test]
async fn bounded_state_identity_does_not_hydrate_large_transcript() {
    let scope = format!("chatgpt:observed:{}", "d".repeat(64));
    let mut store = Store::in_memory().unwrap();
    let conversation_id = store
        .ingest_observation(&Observation {
            provider: "chatgpt".into(),
            account_scope: scope,
            source_id: "large-state-uuid".into(),
            title: "Large state identity".into(),
            url: None,
            current_node: Some("node".into()),
            graph: vec![GraphNode {
                source_node_id: "node".into(),
                source_message_id: Some("message".into()),
                parent_id: None,
                role: Some("assistant".into()),
                author: None,
                create_time: None,
                parts: vec![json!("large transcript text ".repeat(500_000))],
            }],
            raw_payload: Value::Null,
            source_time: None,
            observed_ms: 1,
            parser: "test".into(),
            completeness: "complete".into(),
        })
        .unwrap();
    let app = server::router(store, "state-token".into());
    let response = app
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/conversations/{conversation_id}/state"))
                .header("authorization", "Bearer state-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), axum::http::StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), 64 * 1024)
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["source_id"], "large-state-uuid");
    assert!(bytes.len() < 4096);
}

#[tokio::test]
async fn pin_snapshot_requires_authentication_and_only_complete_verified_lists_apply() {
    let scope = format!("chatgpt:observed:{}", "e".repeat(64));
    let mut store = Store::in_memory().unwrap();
    let source_id = "pin-snapshot-source";
    let conversation_id = store
        .ingest_observation(&Observation {
            provider: "chatgpt".into(),
            account_scope: scope.clone(),
            source_id: source_id.into(),
            title: "Pin snapshot route".into(),
            url: None,
            current_node: None,
            graph: vec![],
            raw_payload: Value::Null,
            source_time: None,
            observed_ms: 1,
            parser: "test".into(),
            completeness: "complete".into(),
        })
        .unwrap();
    let app = server::router(store, "pin-token".into());
    let state_uri = format!("/api/v1/conversations/{conversation_id}/state");
    let receipt = json!({
        "verified": true,
        "conversation_id": conversation_id,
        "source_conversation_id": source_id,
        "scope": scope,
        "target_provider": "chatgpt",
        "target_scope": scope,
        "pinned": true,
    });
    let saved = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(&state_uri)
                .header("authorization", "Bearer pin-token")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({"pinned":true,"receipt":receipt}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(saved.status(), axum::http::StatusCode::OK);

    let unauthenticated = app.clone().oneshot(
        Request::builder()
            .method("POST")
            .uri("/api/v1/chatgpt/pins/snapshot")
            .header("content-type", "application/json")
            .body(Body::from(json!({"receipt":{
                "verified":true,"complete":true,"scope":format!("chatgpt:observed:{}", "e".repeat(64)),"pinned_source_ids":[]
            }}).to_string()))
            .unwrap(),
    ).await.unwrap();
    assert_eq!(
        unauthenticated.status(),
        axum::http::StatusCode::UNAUTHORIZED
    );

    let partial = app.clone().oneshot(
        Request::builder()
            .method("POST")
            .uri("/api/v1/chatgpt/pins/snapshot")
            .header("authorization", "Bearer pin-token")
            .header("content-type", "application/json")
            .body(Body::from(json!({"receipt":{
                "verified":true,"complete":false,"scope":format!("chatgpt:observed:{}", "e".repeat(64)),"pinned_source_ids":[]
            }}).to_string()))
            .unwrap(),
    ).await.unwrap();
    assert_eq!(partial.status(), axum::http::StatusCode::BAD_REQUEST);

    let complete = app.clone().oneshot(
        Request::builder()
            .method("POST")
            .uri("/api/v1/chatgpt/pins/snapshot")
            .header("authorization", "Bearer pin-token")
            .header("content-type", "application/json")
            .body(Body::from(json!({"receipt":{
                "verified":true,"complete":true,"scope":format!("chatgpt:observed:{}", "e".repeat(64)),"pinned_source_ids":[]
            }}).to_string()))
            .unwrap(),
    ).await.unwrap();
    assert_eq!(complete.status(), axum::http::StatusCode::OK);
    let result: Value = serde_json::from_slice(
        &axum::body::to_bytes(complete.into_body(), 64 * 1024)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(result["verified"], true);
    assert_eq!(result["pinned_count"], 0);
    assert_eq!(result["matched_count"], 0);

    let identity = app
        .oneshot(
            Request::builder()
                .uri(&state_uri)
                .header("authorization", "Bearer pin-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let identity: Value = serde_json::from_slice(
        &axum::body::to_bytes(identity.into_body(), 64 * 1024)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(identity["pinned"], false);
}
