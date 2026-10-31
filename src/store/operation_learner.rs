use crate::capture_lane::{CaptureContext, CaptureMetadata};
use rusqlite::{params, OptionalExtension};
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeSet;
use tokio::sync::mpsc;

const MAX_FIELDS: usize = 64;
const ALLOWED_FIELDS: &[&str] = &[
    "gizmo_id",
    "gizmoId",
    "project_id",
    "projectId",
    "conversation_id",
    "conversationId",
    "conversation_ids",
    "conversationIds",
    "id",
    "name",
    "title",
    "display",
    "display_name",
    "displayName",
    "archived",
    "is_archived",
    "isArchived",
    "pinned",
    "is_pinned",
    "isPinned",
    "success",
    "error",
    "code",
    "status",
    "gizmo",
    "project",
];

#[derive(Clone, Debug, Serialize)]
pub(crate) struct OperationObservation {
    pub method: String,
    pub route: String,
    pub status: u16,
    pub request_fields: String,
    pub response_fields: String,
    pub observed_at_ms: i64,
}

#[derive(Clone)]
pub(crate) struct OperationSink {
    tx: mpsc::Sender<OperationObservation>,
    slots: std::sync::Arc<tokio::sync::Semaphore>,
}

pub(crate) fn channel() -> (OperationSink, mpsc::Receiver<OperationObservation>) {
    let (tx, rx) = mpsc::channel(64);
    (
        OperationSink {
            tx,
            slots: std::sync::Arc::new(tokio::sync::Semaphore::new(8)),
        },
        rx,
    )
}

pub(crate) struct Accumulator {
    context: CaptureContext,
    metadata: CaptureMetadata,
    sink: OperationSink,
    response: Vec<u8>,
    overflow: bool,
    _permit: tokio::sync::OwnedSemaphorePermit,
}

impl OperationSink {
    pub(crate) fn is_candidate_request(method: &str, path: &str) -> bool {
        let method = method.to_ascii_uppercase();
        let path = path.split('?').next().unwrap_or_default();
        if !matches!(method.as_str(), "PATCH" | "PUT" | "POST" | "DELETE") {
            return false;
        }
        let Some(rest) = path.strip_prefix("/backend-api/") else {
            return false;
        };
        if let Some(id) = rest.strip_prefix("conversation/") {
            return method == "PATCH" && valid_uuid(id);
        }
        rest == "gizmo"
            || rest.starts_with("gizmo/")
            || rest == "gizmos"
            || rest.starts_with("gizmos/")
            || rest == "project"
            || rest.starts_with("project/")
            || rest == "projects"
            || rest.starts_with("projects/")
    }

    pub(crate) fn begin(
        &self,
        context: CaptureContext,
        metadata: CaptureMetadata,
    ) -> Option<Accumulator> {
        if !metadata.host.eq_ignore_ascii_case("chatgpt.com")
            || !Self::is_candidate_request(&context.method, &context.path)
            || (matches!(
                context.method.to_ascii_uppercase().as_str(),
                "PATCH" | "PUT" | "POST"
            ) && context.request_body.is_none())
            || metadata
                .content_encoding
                .as_deref()
                .is_some_and(|encoding| {
                    !matches!(
                        encoding.trim().to_ascii_lowercase().as_str(),
                        "" | "identity" | "gzip" | "deflate"
                    )
                })
        {
            return None;
        }
        let response_type = metadata
            .content_type
            .as_deref()
            .and_then(|v| v.split(';').next())
            .map(str::trim);
        if response_type.is_some_and(|value| !value.eq_ignore_ascii_case("application/json")) {
            return None;
        }
        let permit = self.slots.clone().try_acquire_owned().ok()?;
        Some(Accumulator {
            context,
            metadata,
            sink: self.clone(),
            response: Vec::new(),
            overflow: false,
            _permit: permit,
        })
    }
}

impl Accumulator {
    pub(crate) fn observe(&mut self, bytes: &[u8]) {
        if self.overflow {
            return;
        }
        if self.response.len().saturating_add(bytes.len()) > 64 * 1024 {
            self.response.clear();
            self.overflow = true;
        } else {
            self.response.extend_from_slice(bytes);
        }
    }

    pub(crate) fn finish(mut self) {
        if self.overflow {
            return;
        }
        let request_fields = if let Some(capture) = &self.context.request_body {
            let mut capture = capture.lock().unwrap_or_else(|p| p.into_inner());
            let Some(body) = capture.body() else {
                capture.clear();
                return;
            };
            let Some(fields) = field_schema(body) else {
                capture.clear();
                return;
            };
            capture.clear();
            fields
        } else {
            "[]".into()
        };
        let Some(route) =
            normalized_route(&self.context.method, &self.context.path, &request_fields)
        else {
            return;
        };
        let mut response_body = crate::live_capture::decode_schema_body(
            self.metadata
                .content_encoding
                .as_deref()
                .unwrap_or("identity"),
            &self.response,
            64 * 1024,
        );
        let response_fields = response_body
            .as_deref()
            .and_then(field_schema)
            .unwrap_or_else(|| "[]".into());
        if let Some(body) = response_body.as_mut() {
            body.fill(0);
        }
        self.response.fill(0);
        self.response.clear();
        drop(response_body);
        let observed_at_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            .min(i64::MAX as u128) as i64;
        let observation = OperationObservation {
            method: self.context.method.to_ascii_uppercase(),
            route,
            status: self.metadata.status,
            request_fields,
            response_fields,
            observed_at_ms,
        };
        let _ = self.sink.tx.try_send(observation);
    }

    pub(crate) fn interrupt(self) {}
}

impl Drop for Accumulator {
    fn drop(&mut self) {
        self.response.fill(0);
        if let Some(capture) = &self.context.request_body {
            capture.lock().unwrap_or_else(|p| p.into_inner()).clear();
        }
    }
}

#[derive(Serialize)]
struct FieldShape<'a> {
    path: &'a str,
    value_type: &'a str,
}

/// Return field names and JSON types only. Unknown keys and all values are discarded.
pub(crate) fn field_schema(bytes: &[u8]) -> Option<String> {
    let value: Value = serde_json::from_slice(bytes).ok()?;
    let mut fields = BTreeSet::<(String, String)>::new();
    collect_fields(&value, "", 0, &mut fields);
    let result: Vec<_> = fields
        .iter()
        .take(MAX_FIELDS)
        .map(|(path, value_type)| FieldShape { path, value_type })
        .collect();
    serde_json::to_string(&result)
        .ok()
        .filter(|text| text.len() <= 8192)
}

fn collect_fields(
    value: &Value,
    prefix: &str,
    depth: usize,
    output: &mut BTreeSet<(String, String)>,
) {
    if depth >= 4 || output.len() >= MAX_FIELDS {
        return;
    }
    let Some(object) = value.as_object() else {
        return;
    };
    for (key, child) in object {
        if !ALLOWED_FIELDS.contains(&key.as_str()) {
            continue;
        }
        let path = if prefix.is_empty() {
            key.to_owned()
        } else {
            format!("{prefix}.{key}")
        };
        let value_type = match child {
            Value::Null => "null",
            Value::Bool(_) => "boolean",
            Value::Number(_) => "number",
            Value::String(_) => "string",
            Value::Array(_) => "array",
            Value::Object(_) => "object",
        };
        output.insert((path.clone(), value_type.to_owned()));
        if child.is_object() {
            collect_fields(child, &path, depth + 1, output);
        }
    }
}

pub(crate) fn normalized_route(method: &str, path: &str, request_fields: &str) -> Option<String> {
    let method = method.to_ascii_uppercase();
    if !matches!(method.as_str(), "PATCH" | "PUT" | "POST" | "DELETE") {
        return None;
    }
    let path = path.split('?').next()?;
    if path.len() > 2048 || !path.starts_with("/backend-api/") {
        return None;
    }
    let pieces: Vec<_> = path.split('/').skip(1).collect();
    if pieces.len() < 2 || pieces.len() > 12 {
        return None;
    }
    if pieces[1] == "conversation" {
        if method != "PATCH" || !valid_uuid(pieces.get(2)?) || pieces.len() != 3 {
            return None;
        }
        let fields: Value = serde_json::from_str(request_fields).ok()?;
        if !fields
            .as_array()?
            .iter()
            .any(|item| item.get("path").and_then(Value::as_str) == Some("gizmo_id"))
        {
            return None;
        }
        return Some("/backend-api/conversation/:conversation_id".into());
    }
    if !matches!(pieces[1], "gizmo" | "gizmos" | "project" | "projects") {
        return None;
    }
    let mut route = String::from("/backend-api");
    for (index, part) in pieces.iter().skip(1).enumerate() {
        if part.is_empty() {
            return None;
        }
        route.push('/');
        if index == 0 || is_static_segment(part) {
            route.push_str(part);
        } else {
            route.push_str(":id");
        }
    }
    (route.len() <= 512).then_some(route)
}

fn is_static_segment(value: &str) -> bool {
    matches!(
        value,
        "snorlax"
            | "sidebar"
            | "rename"
            | "move"
            | "archive"
            | "unarchive"
            | "pin"
            | "unpin"
            | "favorite"
            | "unfavorite"
            | "share"
            | "settings"
            | "update"
    )
}

fn valid_uuid(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(index, byte)| {
            if [8, 13, 18, 23].contains(&index) {
                byte == b'-'
            } else {
                byte.is_ascii_hexdigit()
            }
        })
}

impl super::Store {
    pub(crate) fn record_project_operation(
        &mut self,
        observation: &OperationObservation,
    ) -> rusqlite::Result<()> {
        self.connection.execute_batch(
            "CREATE TABLE IF NOT EXISTS learned_project_operations(\
                id INTEGER PRIMARY KEY,\
                method TEXT NOT NULL, route TEXT NOT NULL, status INTEGER NOT NULL,\
                request_fields TEXT NOT NULL, response_fields TEXT NOT NULL,\
                first_seen_ms INTEGER NOT NULL, last_seen_ms INTEGER NOT NULL,\
                seen_count INTEGER NOT NULL DEFAULT 1,\
                UNIQUE(method,route,status,request_fields,response_fields)\
            );",
        )?;
        self.connection.execute(
            "INSERT INTO learned_project_operations(method,route,status,request_fields,response_fields,first_seen_ms,last_seen_ms,seen_count)\
             VALUES(?1,?2,?3,?4,?5,?6,?6,1)\
             ON CONFLICT(method,route,status,request_fields,response_fields)\
             DO UPDATE SET last_seen_ms=excluded.last_seen_ms,seen_count=min(learned_project_operations.seen_count+1,9223372036854775807)",
            params![observation.method, observation.route, observation.status, observation.request_fields, observation.response_fields, observation.observed_at_ms],
        )?;
        self.connection.execute(
            "DELETE FROM learned_project_operations WHERE id NOT IN (SELECT id FROM learned_project_operations ORDER BY last_seen_ms DESC,id DESC LIMIT 100)",
            [],
        )?;
        Ok(())
    }

    pub(crate) fn learned_project_operations(&self) -> rusqlite::Result<Value> {
        let exists: Option<i64> = self.connection.query_row(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name='learned_project_operations'",
            [], |row| row.get(0),
        ).optional()?;
        if exists.is_none() {
            return Ok(serde_json::json!({"operations":[]}));
        }
        let mut statement = self.connection.prepare(
            "SELECT method,route,status,request_fields,response_fields,first_seen_ms,last_seen_ms,seen_count \
             FROM learned_project_operations ORDER BY last_seen_ms DESC,method,route LIMIT 100",
        )?;
        let rows = statement.query_map([], |row| {
            Ok(serde_json::json!({
                "method":row.get::<_,String>(0)?,"route":row.get::<_,String>(1)?,"status":row.get::<_,u16>(2)?,
                "request_fields":serde_json::from_str::<Value>(&row.get::<_,String>(3)?).unwrap_or(Value::Null),
                "response_fields":serde_json::from_str::<Value>(&row.get::<_,String>(4)?).unwrap_or(Value::Null),
                "first_seen_ms":row.get::<_,i64>(5)?,"last_seen_ms":row.get::<_,i64>(6)?,"seen_count":row.get::<_,u64>(7)?,
            }))
        })?.collect::<Result<Vec<_>,_>>()?;
        Ok(serde_json::json!({"operations":rows}))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[test]
    fn field_schema_discards_values_and_unapproved_keys() {
        let input = br#"{"gizmo_id":"g-p-CANARY_ID","name":"PRIVATE_PROJECT_TITLE","authorization":"Bearer TOKEN_CANARY","cookie":"COOKIE_CANARY","access_token":"ACCESS_TOKEN_CANARY","messages":[{"content":"TRANSCRIPT_CANARY"}],"gizmo":{"name":"INNER_TITLE"}}"#;
        let schema = field_schema(input).unwrap();
        assert!(schema.contains("gizmo_id"));
        assert!(schema.contains("gizmo.name"));
        for secret in [
            "CANARY_ID",
            "PRIVATE_PROJECT_TITLE",
            "TOKEN_CANARY",
            "COOKIE_CANARY",
            "ACCESS_TOKEN_CANARY",
            "TRANSCRIPT_CANARY",
            "INNER_TITLE",
            "authorization",
            "cookie",
            "access_token",
            "messages",
        ] {
            assert!(!schema.contains(secret));
        }
    }

    #[test]
    fn routes_normalize_identifiers_and_require_project_assignment_field() {
        let schema = field_schema(br#"{"gizmo_id":"g-p-secret"}"#).unwrap();
        assert_eq!(
            normalized_route(
                "PATCH",
                "/backend-api/conversation/123e4567-e89b-12d3-a456-426614174000?x=secret",
                &schema
            )
            .as_deref(),
            Some("/backend-api/conversation/:conversation_id")
        );
        assert_eq!(
            normalized_route(
                "PATCH",
                "/backend-api/conversation/123e4567-e89b-12d3-a456-426614174000",
                "[]"
            ),
            None
        );
        assert_eq!(
            normalized_route("POST", "/backend-api/gizmos/g-p-SECRET/rename", "[]").as_deref(),
            Some("/backend-api/gizmos/:id/rename")
        );
    }

    #[test]
    fn persistence_aggregates_only_sanitized_operation_schemas() {
        let mut store = crate::store::Store::in_memory().unwrap();
        let request_fields =
            field_schema(br#"{"gizmo_id":"PRIVATE_ID","name":"PRIVATE_NAME"}"#).unwrap();
        let observation = OperationObservation {
            method: "PATCH".into(),
            route: "/backend-api/gizmos/:id".into(),
            status: 200,
            request_fields,
            response_fields: field_schema(br#"{"success":true,"message":"PRIVATE_RESPONSE_TEXT","authorization":"TOKEN_CANARY"}"#).unwrap(),
            observed_at_ms: 123,
        };
        store.record_project_operation(&observation).unwrap();
        store.record_project_operation(&observation).unwrap();
        let value = store.learned_project_operations().unwrap();
        assert_eq!(value["operations"][0]["seen_count"], 2);
        let summary = value.to_string();
        assert!(summary.contains("seen_count"));
        for secret in [
            "PRIVATE_ID",
            "PRIVATE_NAME",
            "PRIVATE_RESPONSE_TEXT",
            "TOKEN_CANARY",
        ] {
            assert!(!summary.contains(secret));
        }
        assert!(summary.contains("gizmo_id"));
    }

    #[test]
    fn learned_operations_are_capped_to_one_hundred_aggregates() {
        let mut store = crate::store::Store::in_memory().unwrap();
        let observation = OperationObservation {
            method: "PATCH".into(),
            route: "/backend-api/gizmos/:id".into(),
            status: 200,
            request_fields: "[]".into(),
            response_fields: "[]".into(),
            observed_at_ms: 1,
        };
        for status in 0..101 {
            store
                .record_project_operation(&OperationObservation {
                    status,
                    ..observation.clone()
                })
                .unwrap();
        }
        assert_eq!(
            store.learned_project_operations().unwrap()["operations"]
                .as_array()
                .unwrap()
                .len(),
            100
        );
    }

    #[tokio::test]
    async fn oversized_response_is_discarded_without_queueing_partial_shape() {
        let (sink, mut receiver) = channel();
        let context = CaptureContext::at_entry(
            "chatgpt:observed:test",
            "DELETE",
            "/backend-api/gizmos/g-p-private",
        );
        let metadata = CaptureMetadata {
            host: "chatgpt.com".into(),
            status: 200,
            content_type: Some("application/json".into()),
            content_encoding: Some("identity".into()),
        };
        let mut accumulator = sink.begin(context, metadata).unwrap();
        accumulator.observe(&vec![b'x'; 64 * 1024 + 1]);
        accumulator.finish();
        assert!(receiver.try_recv().is_err());
    }

    #[tokio::test]
    async fn conversation_move_capture_queues_only_schema_and_normalized_route() {
        let (sink, mut receiver) = channel();
        let permit = Arc::new(tokio::sync::Semaphore::new(1))
            .acquire_owned()
            .await
            .unwrap();
        let body = Arc::new(Mutex::new(crate::capture_lane::RequestBodyCapture::new(
            permit,
            64 * 1024,
        )));
        {
            let mut request = body.lock().unwrap();
            request
                .observe(br#"{"gizmo_id":"g-p-PRIVATE_PROJECT_ID","name":"PRIVATE_PROJECT_NAME"}"#);
            request.complete();
        }
        let mut context = CaptureContext::at_entry(
            "chatgpt:observed:test",
            "PATCH",
            "/backend-api/conversation/123e4567-e89b-12d3-a456-426614174000?private_query=SECRET",
        );
        context.request_body = Some(body);
        let metadata = CaptureMetadata {
            host: "chatgpt.com".into(),
            status: 200,
            content_type: Some("application/json".into()),
            content_encoding: Some("identity".into()),
        };
        let mut accumulator = sink.begin(context, metadata).unwrap();
        accumulator.observe(br#"{"success":true,"message":"PRIVATE_RESPONSE_TEXT"}"#);
        accumulator.finish();
        let observation = receiver.try_recv().unwrap();
        let text = serde_json::to_string(&observation).unwrap();
        assert_eq!(
            observation.route,
            "/backend-api/conversation/:conversation_id"
        );
        for secret in [
            "PRIVATE_PROJECT_ID",
            "PRIVATE_PROJECT_NAME",
            "PRIVATE_RESPONSE_TEXT",
            "SECRET",
            "123e4567-e89b-12d3-a456-426614174000",
        ] {
            assert!(!text.contains(secret));
        }
        assert!(text.contains("gizmo_id"));
        assert!(text.contains("string"));
    }
}
