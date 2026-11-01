use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
    Router,
};
use chat_hub::{
    catalog::CatalogEntityKind,
    server,
    store::{GraphNode, Observation, ProjectMembership, Store},
};
use hyper::body::Frame;
use serde_json::{json, Value};
use std::{
    convert::Infallible,
    pin::Pin,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    task::{Context, Poll},
};
use tower::ServiceExt;

struct PollCounterBody {
    polled: Arc<AtomicBool>,
    finished: bool,
}
impl hyper::body::Body for PollCounterBody {
    type Data = axum::body::Bytes;
    type Error = Infallible;
    fn poll_frame(
        mut self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        self.polled.store(true, Ordering::SeqCst);
        if self.finished {
            Poll::Ready(None)
        } else {
            self.finished = true;
            Poll::Ready(Some(Ok(Frame::data(axum::body::Bytes::from_static(
                b"untrusted body",
            )))))
        }
    }
}

fn seeded_app() -> (Router, String, String) {
    let mut store = Store::in_memory().unwrap();
    let conversation_id = store
        .ingest_observation(&Observation {
            provider: "chatgpt".into(),
            account_scope: "api-file-account".into(),
            source_id: "api-file-source".into(),
            title: "API file test".into(),
            url: None,
            current_node: Some("assistant".into()),
            graph: vec![GraphNode {
                source_node_id: "assistant".into(),
                source_message_id: Some("source-message".into()),
                parent_id: None,
                role: Some("assistant".into()),
                author: None,
                create_time: None,
                parts: vec![Value::String("sandbox:/mnt/data/api-report.csv".into())],
            }],
            raw_payload: json!({}),
            source_time: None,
            observed_ms: 1,
            parser: "test".into(),
            completeness: "complete".into(),
        })
        .unwrap();
    store.rebuild_catalog_index().unwrap();
    let entity_id = store
        .catalog_entities(
            "chatgpt",
            None,
            Some(&conversation_id),
            Some(CatalogEntityKind::FileArtifact),
            None,
            None,
            10,
        )
        .unwrap()
        .items
        .remove(0)
        .id;
    (
        server::router(store, "secret".into()),
        conversation_id,
        entity_id,
    )
}

async fn send(
    app: &Router,
    method: &str,
    uri: &str,
    auth: bool,
    body: Body,
) -> axum::response::Response {
    let parsed_uri = uri
        .parse::<axum::http::Uri>()
        .unwrap_or_else(|error| panic!("invalid test URI {uri:?}: {error}"));
    let mut request = Request::builder().method(method).uri(parsed_uri);
    if auth {
        request = request.header("authorization", "Bearer secret");
    }
    app.clone()
        .oneshot(request.body(body).unwrap())
        .await
        .unwrap()
}

async fn send_json(
    app: &Router,
    method: &str,
    uri: &str,
    payload: Value,
) -> axum::response::Response {
    let request = Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", "Bearer secret")
        .header("content-type", "application/json")
        .body(Body::from(payload.to_string()))
        .unwrap();
    app.clone().oneshot(request).await.unwrap()
}

fn path_id(value: &str) -> String {
    url::form_urlencoded::byte_serialize(value.as_bytes()).collect()
}

#[tokio::test]
async fn authenticated_file_save_lists_downloads_and_projects_metadata_only() {
    let (app, conversation_id, entity_id) = seeded_app();
    let save_uri = format!("/api/v1/artifacts/{}/content?conversation_id={}&filename=api-report.csv&mime_type=text%2Fcsv", path_id(&entity_id), path_id(&conversation_id));
    let saved = send(&app, "POST", &save_uri, true, Body::from("a,b\n1,2\n")).await;
    assert_eq!(saved.status(), StatusCode::OK);
    let saved_json: Value =
        serde_json::from_slice(&to_bytes(saved.into_body(), 100_000).await.unwrap()).unwrap();
    assert_eq!(saved_json["byte_size"], 8);
    assert!(saved_json["sha256"].as_str().unwrap().len() == 64);
    assert!(saved_json.get("content").is_none());

    let files = send(
        &app,
        "GET",
        "/api/v1/artifacts?limit=10",
        true,
        Body::empty(),
    )
    .await;
    assert_eq!(files.status(), StatusCode::OK);
    let files_json: Value =
        serde_json::from_slice(&to_bytes(files.into_body(), 100_000).await.unwrap()).unwrap();
    assert_eq!(files_json["items"].as_array().unwrap().len(), 1);
    assert!(files_json["items"][0].get("content").is_none());

    let entities = send(
        &app,
        "GET",
        "/api/v1/catalog/entities?source=chatgpt&kind=file_artifact&artifact_state=saved_locally",
        true,
        Body::empty(),
    )
    .await;
    assert_eq!(entities.status(), StatusCode::OK);
    let entity_page: Value =
        serde_json::from_slice(&to_bytes(entities.into_body(), 100_000).await.unwrap()).unwrap();
    assert_eq!(entity_page["items"][0]["id"], entity_id);
    assert_eq!(entity_page["items"][0]["content_saved"], true);
    assert!(entity_page["items"][0].get("content").is_none());

    let download = send(
        &app,
        "GET",
        &format!("/api/v1/artifacts/{}/content", path_id(&entity_id)),
        true,
        Body::empty(),
    )
    .await;
    assert_eq!(download.status(), StatusCode::OK);
    assert_eq!(download.headers()["cache-control"], "no-store");
    assert_eq!(
        to_bytes(download.into_body(), 100_000).await.unwrap(),
        "a,b\n1,2\n"
    );
}

#[tokio::test]
async fn entity_graph_returns_bounded_exact_file_references_with_chat_scope() {
    let (app, conversation_id, entity_id) = seeded_app();
    let encoded_conversation_id: String =
        url::form_urlencoded::byte_serialize(conversation_id.as_bytes()).collect();
    let uri = format!(
        "/api/v1/catalog/entity-graph?source=chatgpt&conversation_id={encoded_conversation_id}&kind=file_artifact&limit=40"
    );
    let response = send(&app, "GET", &uri, true, Body::empty()).await;
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = serde_json::from_slice(
        &to_bytes(response.into_body(), 512 * 1024).await.unwrap(),
    )
    .unwrap();
    assert_eq!(body["total_entities"], 1);
    assert_eq!(body["conversation_id"], conversation_id);
    assert_eq!(body["nodes"].as_array().unwrap().len(), 2);
    let entity = body["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["id"] == entity_id)
        .unwrap();
    assert_eq!(entity["kind"], "file_artifact");
    assert_eq!(entity["entity"]["artifact_state"], "link_only");
    assert_eq!(body["edges"].as_array().unwrap().len(), 1);
    assert_eq!(body["edges"][0]["basis"], "entity_reference");
    assert_eq!(body["edges"][0]["relation"], "mentions");

    let suggestions_uri = format!(
        "/api/v1/catalog/tag-suggestions?conversation_id={encoded_conversation_id}&limit=12"
    );
    let suggestions = send(&app, "GET", &suggestions_uri, true, Body::empty()).await;
    assert_eq!(suggestions.status(), StatusCode::OK);
    let suggestions: Value = serde_json::from_slice(
        &to_bytes(suggestions.into_body(), 64 * 1024).await.unwrap(),
    )
    .unwrap();
    assert!(suggestions["library_revision"].is_number());
    assert!(suggestions["items"].as_array().is_some());

    let empty = send(
        &app,
        "GET",
        "/api/v1/catalog/entity-graph?source=chatgpt&conversation_id=missing&kind=file_artifact",
        true,
        Body::empty(),
    )
    .await;
    assert_eq!(empty.status(), StatusCode::OK);
    let empty: Value = serde_json::from_slice(
        &to_bytes(empty.into_body(), 64 * 1024).await.unwrap(),
    )
    .unwrap();
    assert_eq!(empty["total_entities"], 0);
    assert!(empty["nodes"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn verified_title_commit_targets_an_exact_legacy_paseo_project_row() {
    let mut store = Store::in_memory().unwrap();
    store
        .ingest_observation(&Observation {
            provider: "paseo-chatgpt".into(),
            account_scope: "paseo-cache:owner-123".into(),
            source_id: "legacy-chat-1".into(),
            title: "Cached conversation".into(),
            url: None,
            current_node: None,
            graph: vec![],
            raw_payload: json!({}),
            source_time: None,
            observed_ms: 1,
            parser: "test".into(),
            completeness: "complete".into(),
        })
        .unwrap();
    let project_key = "project:[\"paseo-chatgpt\",\"paseo-cache:owner-123\",\"g-p-shared_123\"]";
    store
        .apply_project_memberships(
            "paseo-chatgpt",
            "paseo-cache:owner-123",
            &[ProjectMembership {
                source_id: "legacy-chat-1".into(),
                project_id: Some("g-p-shared_123".into()),
                title: Some("Cached title".into()),
            }],
        )
        .unwrap();
    let app = server::router(store, "secret".into());
    let expected_scope = format!("chatgpt:observed:{}", "a".repeat(64));

    let wrong_target = send_json(
        &app,
        "PATCH",
        "/api/v1/projects/title",
        json!({
            "project_key": project_key,
            "expected_scope": expected_scope,
            "title": "Must not change",
            "legacy_target": {
                "account_scope": "paseo-cache:other-owner",
                "source_id": "g-p-shared_123"
            }
        }),
    )
    .await;
    assert_eq!(wrong_target.status(), StatusCode::CONFLICT);

    let expected_scope = format!("chatgpt:observed:{}", "a".repeat(64));
    let updated = send_json(
        &app,
        "PATCH",
        "/api/v1/projects/title",
        json!({
            "project_key": project_key,
            "expected_scope": expected_scope,
            "title": "Verified Chat Hub title",
            "legacy_target": {
                "account_scope": "paseo-cache:owner-123",
                "source_id": "g-p-shared_123"
            }
        }),
    )
    .await;
    assert_eq!(updated.status(), StatusCode::OK);
    let payload: Value =
        serde_json::from_slice(&to_bytes(updated.into_body(), 100_000).await.unwrap()).unwrap();
    assert_eq!(payload["verified"], true);
    assert_eq!(payload["project"]["project_key"], project_key);
    assert_eq!(
        payload["project"]["display_name"],
        "Verified Chat Hub title"
    );
}

#[tokio::test]
async fn simultaneous_entity_and_tag_reads_do_not_contend_for_the_writer_slot() {
    let (app, _conversation_id, entity_id) = seeded_app();
    let tag_uri = format!(
        "/api/v1/tags/assignments?target_kind=file&target_id={}",
        path_id(&entity_id)
    );
    let entity_uri = format!("/api/v1/catalog/entities/{}", path_id(&entity_id));
    let (tags, target_tags, entity) = tokio::join!(
        send(
            &app,
            "GET",
            "/api/v1/tags?target_kind=file",
            true,
            Body::empty()
        ),
        send(&app, "GET", &tag_uri, true, Body::empty()),
        send(&app, "GET", &entity_uri, true, Body::empty()),
    );
    assert_eq!(tags.status(), StatusCode::OK);
    assert_eq!(target_tags.status(), StatusCode::OK);
    assert_eq!(entity.status(), StatusCode::OK);
}

#[tokio::test]
async fn unauthenticated_upload_is_rejected_without_polling_its_body() {
    let (app, conversation_id, entity_id) = seeded_app();
    let polled = Arc::new(AtomicBool::new(false));
    let body = Body::new(PollCounterBody {
        polled: polled.clone(),
        finished: false,
    });
    let uri = format!(
        "/api/v1/artifacts/{}/content?conversation_id={}&filename=stolen.csv",
        path_id(&entity_id),
        path_id(&conversation_id)
    );
    let response = send(&app, "POST", &uri, false, body).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert!(
        !polled.load(Ordering::SeqCst),
        "authentication must reject before reading upload bytes"
    );
}
