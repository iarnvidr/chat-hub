//! Authenticated local catalog, tag, and saved-file operations.
use super::*;
use crate::{
    artifacts::MAX_ARTIFACT_BYTES,
    catalog::{CatalogEntityGraphQuery, CatalogEntityKind},
    store::Store,
};
use axum::extract::DefaultBodyLimit;

pub(super) fn routes() -> Router<ApiState> {
    Router::new()
        .route("/api/v1/catalog/entities", get(entities))
        .route("/api/v1/catalog/entities/:id", get(entity))
        .route("/api/v1/catalog/entity-graph", get(entity_graph))
        .route("/api/v1/catalog/tag-suggestions", get(tag_suggestions))
        .route("/api/v1/tags", get(tags))
        .route("/api/v1/tags/assignments", get(target_tags).put(set_tags))
        .route("/api/v1/tags/targets", get(tagged_targets))
        .route("/api/v1/artifacts", get(saved_files))
        .route("/api/v1/artifacts/:id", get(saved_file))
        .route(
            "/api/v1/artifacts/:id/content",
            get(file_content)
                .post(save_content)
                .layer(DefaultBodyLimit::max(MAX_ARTIFACT_BYTES)),
        )
}

async fn store_op<T, F>(s: &ApiState, operation: F) -> Result<T, ApiError>
where
    T: Send + 'static,
    F: FnOnce(&mut Store) -> crate::error::Result<T> + Send + 'static,
{
    let store = s.import_store.clone().ok_or_else(unavailable)?;
    let db_permit = s
        .db_ops
        .clone()
        .try_acquire_owned()
        .map_err(|_| unavailable())?;
    tokio::task::spawn_blocking(move || {
        let _db_permit = db_permit;
        let mut store = store.lock().map_err(|_| unavailable())?;
        operation(&mut store).map_err(|e| match e {
            crate::error::Error::Configuration(_) => invalid_request(),
            _ => unavailable(),
        })
    })
    .await
    .map_err(|_| unavailable())?
}
async fn store_write_op<T, F>(s: &ApiState, operation: F) -> Result<T, ApiError>
where
    T: Send + 'static,
    F: FnOnce(&mut Store) -> crate::error::Result<T> + Send + 'static,
{
    let permit = s
        .import_ops
        .clone()
        .try_acquire_owned()
        .map_err(|_| unavailable())?;
    store_op_with_permit(s, permit, operation).await
}
async fn store_op_with_permit<T, F>(
    s: &ApiState,
    permit: tokio::sync::OwnedSemaphorePermit,
    operation: F,
) -> Result<T, ApiError>
where
    T: Send + 'static,
    F: FnOnce(&mut Store) -> crate::error::Result<T> + Send + 'static,
{
    let store = s.import_store.clone().ok_or_else(unavailable)?;
    let db_permit = s
        .db_ops
        .clone()
        .try_acquire_owned()
        .map_err(|_| unavailable())?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let _db_permit = db_permit;
        let mut store = store.lock().map_err(|_| unavailable())?;
        operation(&mut store).map_err(|e| match e {
            crate::error::Error::Configuration(_) => invalid_request(),
            _ => unavailable(),
        })
    })
    .await
    .map_err(|_| unavailable())?
}
fn valid_id(id: &str) -> bool {
    !id.trim().is_empty() && id.len() <= 1024 && !id.chars().any(char::is_control)
}
fn as_json(value: impl Serialize) -> ApiResult {
    checked_json(serde_json::to_value(value).map_err(|_| unavailable())?)
}
fn missing() -> ApiError {
    error(
        StatusCode::NOT_FOUND,
        "not_found",
        "The item was not found.",
    )
}

#[derive(Deserialize)]
struct EntityQuery {
    source: Option<String>,
    project_key: Option<String>,
    kind: Option<String>,
    artifact_state: Option<String>,
    conversation_id: Option<String>,
    tag: Option<String>,
    query: Option<String>,
    cursor: Option<String>,
    limit: Option<usize>,
}
async fn entities(
    State(s): State<ApiState>,
    q: Result<Query<EntityQuery>, QueryRejection>,
) -> ApiResult {
    let q = query(q)?;
    let source = q.source.unwrap_or_else(|| "all".into());
    validate_catalog_scope(&source, q.project_key.as_deref())?;
    if q.kind
        .as_deref()
        .is_some_and(|k| CatalogEntityKind::parse(k).is_none())
        || q.artifact_state
            .as_deref()
            .is_some_and(|v| !matches!(v, "link_only" | "metadata_present" | "saved_locally"))
        || q.conversation_id.as_deref().is_some_and(|v| !valid_id(v))
        || q.cursor.as_ref().is_some_and(|v| v.len() > 4096)
        || !(1..=100).contains(&q.limit.unwrap_or(40))
    {
        return Err(invalid_request());
    }
    let page = store_op(&s, move |store| {
        store.catalog_entities_search(crate::catalog::CatalogEntityQuery {
            source: &source,
            project_key: q.project_key.as_deref(),
            conversation_id: q.conversation_id.as_deref(),
            kind: q.kind.as_deref().and_then(CatalogEntityKind::parse),
            artifact_state: q.artifact_state.as_deref(),
            tag: q.tag.as_deref(),
            query: q.query.as_deref(),
            cursor: q.cursor.as_deref(),
            limit: q.limit.unwrap_or(40),
        })
    })
    .await?;
    as_json(page)
}
async fn entity(State(s): State<ApiState>, Path(id): Path<String>) -> ApiResult {
    if !valid_id(&id) {
        return Err(invalid_request());
    }
    as_json(
        store_op(&s, move |store| store.catalog_entity(&id))
            .await?
            .ok_or_else(missing)?,
    )
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EntityGraphQuery {
    source: Option<String>,
    project_key: Option<String>,
    conversation_id: Option<String>,
    tag: Option<String>,
    query: Option<String>,
    kind: Option<String>,
    artifact_state: Option<String>,
    limit: Option<usize>,
}

async fn entity_graph(
    State(s): State<ApiState>,
    q: Result<Query<EntityGraphQuery>, QueryRejection>,
) -> ApiResult {
    let q = query(q)?;
    let source = q.source.unwrap_or_else(|| "all".into());
    validate_catalog_scope(&source, q.project_key.as_deref())?;
    if q.conversation_id.as_deref().is_some_and(|value| !valid_id(value))
        || q.query
            .as_deref()
            .is_some_and(|value| value.len() > 256 || value.chars().any(char::is_control))
        || q.kind
            .as_deref()
            .is_some_and(|value| CatalogEntityKind::parse(value).is_none())
        || q.artifact_state
            .as_deref()
            .is_some_and(|value| !matches!(value, "link_only" | "metadata_present" | "saved_locally"))
        || !(1..=40).contains(&q.limit.unwrap_or(40))
    {
        return Err(invalid_request());
    }
    let graph = store_op(&s, move |store| {
        store.catalog_entity_graph(CatalogEntityGraphQuery {
            source: &source,
            project_key: q.project_key.as_deref(),
            conversation_id: q.conversation_id.as_deref(),
            tag: q.tag.as_deref(),
            query: q.query.as_deref(),
            kind: q.kind.as_deref().and_then(CatalogEntityKind::parse),
            artifact_state: q.artifact_state.as_deref(),
            limit: q.limit.unwrap_or(40),
        })
    })
    .await?;
    as_json(graph)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TagSuggestionQuery {
    conversation_id: String,
    limit: Option<usize>,
}

async fn tag_suggestions(
    State(s): State<ApiState>,
    q: Result<Query<TagSuggestionQuery>, QueryRejection>,
) -> ApiResult {
    let q = query(q)?;
    if !valid_id(&q.conversation_id) || !(1..=12).contains(&q.limit.unwrap_or(12)) {
        return Err(invalid_request());
    }
    let suggestions = store_op(&s, move |store| {
        store.catalog_tag_suggestions(&q.conversation_id, q.limit.unwrap_or(12))
    })
    .await?;
    as_json(suggestions)
}

#[derive(Deserialize)]
struct TagQuery {
    target_kind: Option<String>,
    query: Option<String>,
    limit: Option<usize>,
}
async fn tags(State(s): State<ApiState>, q: Result<Query<TagQuery>, QueryRejection>) -> ApiResult {
    let q = query(q)?;
    let items = store_op(&s, move |store| {
        store.list_tags(
            q.target_kind.as_deref(),
            q.query.as_deref(),
            q.limit.unwrap_or(100),
        )
    })
    .await?;
    as_json(json!({"items":items}))
}
#[derive(Deserialize)]
struct TargetQuery {
    target_kind: String,
    target_id: String,
}
async fn target_tags(
    State(s): State<ApiState>,
    q: Result<Query<TargetQuery>, QueryRejection>,
) -> ApiResult {
    let q = query(q)?;
    let kind = q.target_kind.clone();
    let id = q.target_id.clone();
    let tags = store_op(&s, move |store| {
        store.target_tags(&q.target_kind, &q.target_id)
    })
    .await?;
    as_json(json!({"target_kind":kind,"target_id":id,"tags":tags}))
}
#[derive(Deserialize)]
struct SetTags {
    target_kind: String,
    target_id: String,
    tags: Vec<String>,
}
async fn set_tags(State(s): State<ApiState>, Json(request): Json<SetTags>) -> ApiResult {
    if !valid_id(&request.target_id) || request.tags.len() > 64 {
        return Err(invalid_request());
    }
    let result = store_write_op(&s, move |store| {
        if matches!(request.target_kind.as_str(), "entity" | "file") {
            let detail = store.catalog_entity(&request.target_id)?;
            let saved = request.target_kind == "file"
                && store.saved_artifact(&request.target_id)?.is_some();
            let matches = detail.is_some_and(|v| {
                if request.target_kind == "file" {
                    v.entity.kind == CatalogEntityKind::FileArtifact
                } else {
                    v.entity.kind != CatalogEntityKind::FileArtifact
                }
            });
            if !matches && !saved {
                return Err(crate::error::Error::Configuration(
                    "tag target not found".into(),
                ));
            }
        }
        store.set_target_tags(&request.target_kind, &request.target_id, &request.tags)
    })
    .await?;
    as_json(result)
}
#[derive(Deserialize)]
struct TaggedQuery {
    target_kind: Option<String>,
    tag: String,
    limit: Option<usize>,
    after_kind: Option<String>,
    after_id: Option<String>,
    tag_revision: Option<i64>,
}
async fn tagged_targets(
    State(s): State<ApiState>,
    q: Result<Query<TaggedQuery>, QueryRejection>,
) -> ApiResult {
    let q = query(q)?;
    if q.after_kind.is_some() != q.after_id.is_some() {
        return Err(invalid_request());
    }
    if q.tag_revision.is_some() != q.after_kind.is_some()
        || q.tag_revision.is_some_and(|revision| revision < 0)
    {
        return Err(invalid_request());
    }
    let limit = q.limit.unwrap_or(100);
    let expected_revision = q.tag_revision;
    let page = store_op(&s, move |store| {
        store.tagged_targets_snapshot(
            q.target_kind.as_deref(),
            &q.tag,
            limit,
            q.after_kind.as_deref().zip(q.after_id.as_deref()),
        )
    })
    .await?;
    if expected_revision.is_some_and(|revision| revision != page.tag_revision) {
        return Err(error(
            StatusCode::CONFLICT,
            "stale_cursor",
            "Tags changed. Restart from the first page.",
        ));
    }
    let next = if page.items.len() == limit {
        page.items
            .last()
            .map(|v| json!({"target_kind":v.target_kind,"target_id":v.target_id,"tag_revision":page.tag_revision}))
    } else {
        None
    };
    as_json(json!({"items":page.items,"next":next,"tag_revision":page.tag_revision}))
}

#[derive(Deserialize)]
struct FileQuery {
    conversation_id: Option<String>,
    after: Option<String>,
    limit: Option<usize>,
}
async fn saved_files(
    State(s): State<ApiState>,
    q: Result<Query<FileQuery>, QueryRejection>,
) -> ApiResult {
    let q = query(q)?;
    let limit = q.limit.unwrap_or(100);
    let items = store_op(&s, move |store| {
        store.saved_artifacts(q.conversation_id.as_deref(), q.after.as_deref(), limit)
    })
    .await?;
    let next = if items.len() == limit {
        items.last().map(|v| v.entity_id.clone())
    } else {
        None
    };
    as_json(json!({"items":items,"next_cursor":next}))
}
async fn saved_file(State(s): State<ApiState>, Path(id): Path<String>) -> ApiResult {
    if !valid_id(&id) {
        return Err(invalid_request());
    }
    as_json(
        store_op(&s, move |store| store.saved_artifact(&id))
            .await?
            .ok_or_else(missing)?,
    )
}
#[derive(Deserialize)]
struct SaveFileQuery {
    conversation_id: String,
    filename: String,
    mime_type: Option<String>,
}
async fn save_content(
    State(s): State<ApiState>,
    Path(id): Path<String>,
    q: Result<Query<SaveFileQuery>, QueryRejection>,
    request: axum::extract::Request,
) -> ApiResult {
    let q = query(q)?;
    if !valid_id(&id) || !valid_id(&q.conversation_id) {
        return Err(invalid_request());
    }
    // Reserve a bounded writer slot before buffering a potentially large file.
    let permit = s
        .import_ops
        .clone()
        .try_acquire_owned()
        .map_err(|_| unavailable())?;
    let body = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        axum::body::to_bytes(request.into_body(), MAX_ARTIFACT_BYTES),
    )
    .await
    .map_err(|_| invalid_request())?
    .map_err(|_| {
        error(
            StatusCode::PAYLOAD_TOO_LARGE,
            "file_too_large",
            "Files may be up to 64 MiB.",
        )
    })?;
    let receipt = store_op_with_permit(&s, permit, move |store| {
        store.save_artifact_content(
            &id,
            &q.conversation_id,
            &q.filename,
            q.mime_type.as_deref(),
            &body,
        )
    })
    .await?;
    as_json(receipt)
}
async fn file_content(
    State(s): State<ApiState>,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    if !valid_id(&id) {
        return Err(invalid_request());
    }
    let (metadata, bytes) = store_op(&s, move |store| store.artifact_content(&id))
        .await?
        .ok_or_else(missing)?;
    let mut response = bytes.into_response();
    response.headers_mut().insert(
        axum::http::header::CONTENT_TYPE,
        axum::http::HeaderValue::from_static("application/octet-stream"),
    );
    response.headers_mut().insert(
        axum::http::header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("no-store"),
    );
    response.headers_mut().insert(
        "x-content-type-options",
        axum::http::HeaderValue::from_static("nosniff"),
    );
    response.headers_mut().insert(
        "x-artifact-sha256",
        axum::http::HeaderValue::from_str(&metadata.sha256).map_err(|_| unavailable())?,
    );
    Ok(response)
}
