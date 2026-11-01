//! Bounded ChatGPT export ingestion shared by CLI and HTTP adapters.
use serde::Serialize;
use serde_json::Value;
use std::io::Read;

pub const MAX_IMPORT_BYTES: usize = 67_108_864;
pub const MAX_BYTES: usize = MAX_IMPORT_BYTES;
pub const MAX_ENTRIES: usize = 10_000;

#[derive(Debug, Default, Serialize, PartialEq, Eq)]
pub struct ImportReceipt {
    pub imported: usize,
    pub updated: usize,
    pub unchanged: usize,
    pub failed: usize,
    pub errors: Vec<ItemError>,
    pub errors_truncated: bool,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct ItemError {
    pub item_index: usize,
    pub code: &'static str,
    pub message: &'static str,
}

/// Safe admission errors; never include source bytes, file paths or DB diagnostics.
#[derive(Debug, PartialEq, Eq)]
pub struct ImportError {
    pub code: &'static str,
    pub message: &'static str,
}
impl std::fmt::Display for ImportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}
impl std::error::Error for ImportError {}

fn invalid() -> ImportError {
    ImportError {
        code: "invalid_request",
        message: "Expected a UTF-8 JSON conversation array or an object containing conversations.",
    }
}

/// Read at most the limit plus one byte, including when metadata is unavailable.
pub fn read_input(reader: impl Read) -> Result<Vec<u8>, ImportError> {
    let mut bytes = Vec::new();
    reader
        .take(MAX_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| ImportError {
            code: "input_unavailable",
            message: "Unable to read import input.",
        })?;
    check_size(&bytes)?;
    Ok(bytes)
}

fn check_size(bytes: &[u8]) -> Result<(), ImportError> {
    if bytes.len() > MAX_BYTES {
        return Err(ImportError {
            code: "payload_too_large",
            message: "Import exceeds the 64 MiB limit.",
        });
    }
    Ok(())
}

/// Validate the entire global envelope before returning any items for ingestion.
pub fn parse_export(bytes: &[u8]) -> Result<Vec<Value>, ImportError> {
    check_size(bytes)?;
    if bytes.starts_with(b"PK") {
        return Err(ImportError {
            code: "invalid_request",
            message: "Extract conversations.json from the ZIP archive before importing.",
        });
    }
    let text = std::str::from_utf8(bytes).map_err(|_| invalid())?;
    let value: Value = serde_json::from_str(text).map_err(|_| invalid())?;
    let entries = match value {
        Value::Array(entries) => entries,
        Value::Object(mut object) => match object.remove("conversations") {
            Some(Value::Array(entries)) => entries,
            _ => return Err(invalid()),
        },
        _ => return Err(invalid()),
    };
    if entries.len() > MAX_ENTRIES {
        return Err(ImportError {
            code: "invalid_request",
            message: "Import exceeds the 10000 conversation limit.",
        });
    }
    Ok(entries)
}

/// Commit each valid item independently. A disconnected caller cannot undo commits.
pub fn import_chatgpt(
    store: &mut crate::store::Store,
    bytes: &[u8],
) -> Result<ImportReceipt, ImportError> {
    import_scoped(
        store,
        bytes,
        "provisional:default",
        "chatgpt-export-v1",
        false,
    )
}

/// Dedicated trusted integration admission. Ordinary export files cannot select
/// this identity scope merely by including an owner field in their JSON.
pub fn import_paseo_history(
    store: &mut crate::store::Store,
    bytes: &[u8],
) -> Result<ImportReceipt, ImportError> {
    check_size(bytes)?;
    let envelope: Value = serde_json::from_slice(bytes).map_err(|_| invalid())?;
    let owner = envelope
        .get("owner")
        .and_then(Value::as_str)
        .filter(|owner| {
            owner.len() == 24
                && owner
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        })
        .ok_or_else(invalid)?;
    let cached = match envelope.get("format").and_then(Value::as_str) {
        Some("paseo-cached-transcripts-v1") => true,
        Some("chat-hub-paseo-history-v1") | None => false,
        _ => return Err(invalid()),
    };
    let scope = format!("{}:{owner}", if cached { "paseo-cache" } else { "paseo" });
    import_scoped(store, bytes, &scope, "chatgpt-paseo-history-v1", cached)
}

/// App-owned direct history fetching uses the same stable account attribution
/// as live requests when those claims are present. Generic export import must
/// never honor a file's claimed account scope.
pub fn import_chatgpt_history(
    store: &mut crate::store::Store,
    bytes: &[u8],
) -> Result<ImportReceipt, ImportError> {
    check_size(bytes)?;
    let envelope: Value = serde_json::from_slice(bytes).map_err(|_| invalid())?;
    if envelope.get("format").and_then(Value::as_str) != Some("chat-hub-history-v1") {
        return Err(invalid());
    }
    let scope = envelope
        .get("account_scope")
        .and_then(Value::as_str)
        .ok_or_else(invalid)?;
    let hash = scope
        .strip_prefix("chatgpt:observed:")
        .or_else(|| scope.strip_prefix("chatgpt:oauth:"))
        .ok_or_else(invalid)?;
    if hash.len() != 64
        || !hash
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(invalid());
    }
    let catalog = match envelope.get("project_catalog") {
        None => Vec::new(),
        Some(Value::Array(items)) => items.clone(),
        _ => return Err(invalid()),
    };
    let memberships = match envelope.get("conversation_projects") {
        None => Vec::new(),
        Some(Value::Array(items)) => items.clone(),
        _ => return Err(invalid()),
    };
    if catalog.len() > MAX_ENTRIES || memberships.len() > MAX_ENTRIES {
        return Err(invalid());
    }
    let mut titles = std::collections::HashMap::<String, Option<String>>::new();
    for project in catalog {
        let Some(id) = project
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty() && id.len() <= 256)
        else {
            return Err(invalid());
        };
        let title = match project.get("title") {
            None | Some(Value::Null) => None,
            Some(Value::String(title)) if title.len() <= 4096 => {
                let title = title.trim();
                (!title.is_empty()).then(|| title.to_owned())
            }
            _ => return Err(invalid()),
        };
        if titles.insert(id.to_owned(), title).is_some() {
            return Err(invalid());
        }
    }
    let mut mappings = Vec::<crate::store::ProjectMembership>::new();
    let mut seen = std::collections::HashSet::new();
    for membership in memberships {
        let Some(source_id) = membership
            .get("conversation_id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty() && id.len() <= 512)
        else {
            return Err(invalid());
        };
        let project_id = match membership.get("project_id") {
            Some(Value::Null) => None,
            Some(Value::String(id)) if !id.is_empty() && id.len() <= 256 => Some(id.clone()),
            _ => return Err(invalid()),
        };
        if !seen.insert(source_id.to_owned()) {
            return Err(invalid());
        }
        if project_id
            .as_ref()
            .is_some_and(|id| !titles.contains_key(id))
        {
            return Err(invalid());
        }
        mappings.push(crate::store::ProjectMembership {
            source_id: source_id.to_owned(),
            project_id,
            title: None,
        });
    }
    for mapping in &mut mappings {
        mapping.title = mapping
            .project_id
            .as_ref()
            .and_then(|id| titles.get(id))
            .and_then(Clone::clone);
    }
    let project_titles = titles
        .into_iter()
        .map(|(source_id, title)| crate::store::ProjectTitle { source_id, title })
        .collect::<Vec<_>>();
    let entries = envelope
        .get("conversations")
        .and_then(Value::as_array)
        .ok_or_else(invalid)?;
    let (receipt, failed_items) = if entries.is_empty() {
        (ImportReceipt::default(), Vec::new())
    } else {
        import_scoped_detailed(store, bytes, scope, "chatgpt-history-v1", false)?
    };
    let failed_source_ids = failed_items
        .into_iter()
        .filter_map(|index| entries.get(index).and_then(history_source_id))
        .collect::<std::collections::HashSet<_>>();
    let mut applicable_mappings = Vec::with_capacity(mappings.len());
    for mapping in mappings {
        let exists: bool = store.connection().query_row(
            "SELECT EXISTS(SELECT 1 FROM conversations WHERE provider=?1 AND account_scope=?2 AND source_id=?3)",
            rusqlite::params!["chatgpt", scope, mapping.source_id],
            |row| row.get(0),
        ).map_err(|_| invalid())?;
        if exists {
            applicable_mappings.push(mapping);
        } else if !entries.is_empty() && !failed_source_ids.contains(&mapping.source_id) {
            return Err(invalid());
        }
    }
    store
        .apply_project_titles("chatgpt", scope, &project_titles)
        .map_err(|_| invalid())?;
    store
        .apply_project_memberships("chatgpt", scope, &applicable_mappings)
        .map_err(|_| invalid())?;
    Ok(receipt)
}

fn history_source_id(document: &Value) -> Option<String> {
    document
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty() && id.len() <= 512)
        .or_else(|| {
            document
                .get("conversation_id")
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty() && id.len() <= 512)
        })
        .map(ToOwned::to_owned)
}

fn import_scoped(
    store: &mut crate::store::Store,
    bytes: &[u8],
    scope: &str,
    parser: &str,
    cached: bool,
) -> Result<ImportReceipt, ImportError> {
    import_scoped_detailed(store, bytes, scope, parser, cached).map(|(receipt, _)| receipt)
}

fn import_scoped_detailed(
    store: &mut crate::store::Store,
    bytes: &[u8],
    scope: &str,
    parser: &str,
    cached: bool,
) -> Result<(ImportReceipt, Vec<usize>), ImportError> {
    use crate::store::IngestOutcome;
    let entries = parse_export(bytes)?;
    let mut receipt = ImportReceipt::default();
    let mut failed_items = Vec::new();
    for (item_index, doc) in entries.iter().enumerate() {
        let observed_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            .min(i64::MAX as u128) as i64;
        let result = if cached {
            normalize_paseo_cached_transcript(doc, observed_ms, scope)
        } else {
            crate::capture::normalize_chatgpt_document_in_scope(doc, parser, observed_ms, scope)
        };
        let failure = match result {
            Err(_) => Some(("invalid_item", "Conversation entry is invalid.")),
            Ok(observation) => match if cached {
                ingest_cached_if_changed(store, &observation)
            } else {
                store.ingest_observation_outcome(&observation)
            } {
                Ok(IngestOutcome::Imported { .. }) => {
                    receipt.imported += 1;
                    None
                }
                Ok(IngestOutcome::Updated { .. }) => {
                    receipt.updated += 1;
                    None
                }
                Ok(IngestOutcome::Unchanged { .. }) => {
                    receipt.unchanged += 1;
                    None
                }
                Err(_) => Some(("store_failed", "Conversation could not be committed.")),
            },
        };
        if let Some((code, message)) = failure {
            receipt.failed += 1;
            failed_items.push(item_index);
            if receipt.errors.len() < 100 {
                receipt.errors.push(ItemError {
                    item_index,
                    code,
                    message,
                });
            } else {
                receipt.errors_truncated = true;
            }
        }
    }
    Ok((receipt, failed_items))
}

fn ingest_cached_if_changed(
    store: &mut crate::store::Store,
    observation: &crate::store::Observation,
) -> crate::error::Result<crate::store::IngestOutcome> {
    use rusqlite::OptionalExtension;
    let previous: Option<(String, String)> = store.connection().query_row(
        "SELECT c.id,o.raw_payload FROM conversations c JOIN observations o ON o.conversation_id=c.id WHERE c.provider=?1 AND c.account_scope=?2 AND c.source_id=?3 AND o.parser=?4 ORDER BY o.id DESC LIMIT 1",
        rusqlite::params![observation.provider, observation.account_scope, observation.source_id, observation.parser],
        |row| Ok((row.get(0)?, row.get(1)?)),
    ).optional()?;
    if let Some((conversation_id, payload)) = previous {
        if payload == observation.raw_payload {
            return Ok(crate::store::IngestOutcome::Unchanged { conversation_id });
        }
    }
    store.ingest_observation_outcome(observation)
}

fn normalize_paseo_cached_transcript(
    doc: &Value,
    observed_ms: i64,
    scope: &str,
) -> crate::error::Result<crate::store::Observation> {
    let invalid = || crate::error::Error::Configuration("Invalid Paseo cached conversation".into());
    let source = doc
        .get("source")
        .and_then(Value::as_str)
        .filter(|source| matches!(*source, "chatgpt" | "codex_local" | "codex_cloud"))
        .ok_or_else(invalid)?;
    if doc.get("kind").and_then(Value::as_str) != Some("conversation") {
        return Err(invalid());
    }
    let id = doc
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .ok_or_else(invalid)?;
    let text = doc
        .get("text")
        .and_then(Value::as_str)
        .ok_or_else(invalid)?;
    // The plugin cache contains a rendered selected transcript, not the original
    // message graph. Preserve it as one archive record; never invent user/assistant
    // boundaries by parsing headings that may also occur inside message content.
    let mut graph = serde_json::json!({
        "id": id, "title": doc.get("title").and_then(Value::as_str).unwrap_or("Saved conversation"),
        "current_node": "paseo-cached-transcript", "paseo_cache": doc,
        "mapping": {"paseo-cached-transcript": {
            "id": "paseo-cached-transcript", "parent": null, "children": [],
            "message": {"id":"paseo-cached-transcript", "author":{"role":"archive"},
                "content":{"content_type":"text", "parts":[text]}}
        }}
    });
    if let Some(updated) = doc.get("updated").and_then(Value::as_str) {
        let seconds = updated
            .parse::<f64>()
            .ok()
            .filter(|value| value.is_finite())
            .or_else(|| {
                time::OffsetDateTime::parse(updated, &time::format_description::well_known::Rfc3339)
                    .ok()
                    .map(|value| value.unix_timestamp_nanos() as f64 / 1_000_000_000.0)
            });
        if let Some(value) = seconds.and_then(serde_json::Number::from_f64) {
            graph["update_time"] = Value::Number(value);
        }
    }
    let mut observation = crate::capture::normalize_chatgpt_document_in_scope(
        &graph,
        "paseo-cache-transcript-v1",
        observed_ms,
        scope,
    )?;
    observation.provider = format!("paseo-{source}");
    observation.url = doc.get("url").and_then(Value::as_str).map(str::to_owned);
    observation.completeness = "cached_transcript".into();
    Ok(observation)
}

#[cfg(test)]
mod project_metadata_import_tests {
    use super::*;

    fn seed(store: &mut crate::store::Store, source_id: &str) {
        let scope = format!("chatgpt:observed:{}", "a".repeat(64));
        store
            .ingest_observation(&crate::store::Observation {
                provider: "chatgpt".into(),
                account_scope: scope,
                source_id: source_id.into(),
                title: "Seed conversation".into(),
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
    }

    #[test]
    fn metadata_only_envelope_updates_project_and_revision_without_graph_import() {
        let mut store = crate::store::Store::in_memory().unwrap();
        seed(&mut store, "existing");
        let scope = format!("chatgpt:observed:{}", "a".repeat(64));
        let body = serde_json::json!({
            "format":"chat-hub-history-v1", "account_scope":scope, "conversations":[],
            "project_catalog":[{"id":"project-1","title":"Project title"}],
            "conversation_projects":[{"conversation_id":"existing","project_id":"project-1"}]
        });
        let before = store.library_revision().unwrap();
        let receipt =
            import_chatgpt_history(&mut store, &serde_json::to_vec(&body).unwrap()).unwrap();
        assert_eq!(
            receipt.imported + receipt.updated + receipt.unchanged + receipt.failed,
            0
        );
        assert!(store.library_revision().unwrap() > before);
        let projects = store.projects_snapshot("chatgpt").unwrap();
        assert_eq!(projects.projects[0]["title"], "Project title");
        assert_eq!(projects.projects[0]["conversation_count"], 1);
    }

    #[test]
    fn sidebar_only_envelope_updates_preexisting_project_title_without_membership_fetch() {
        let mut store = crate::store::Store::in_memory().unwrap();
        seed(&mut store, "existing");
        store
            .apply_project_memberships(
                "chatgpt",
                &format!("chatgpt:observed:{}", "a".repeat(64)),
                &[crate::store::ProjectMembership {
                    source_id: "existing".into(),
                    project_id: Some("project-1".into()),
                    title: None,
                }],
            )
            .unwrap();
        let scope = format!("chatgpt:observed:{}", "a".repeat(64));
        let body = serde_json::json!({
            "format":"chat-hub-history-v1", "account_scope":scope, "conversations":[],
            "project_catalog":[{"id":"project-1","title":"Current sidebar name"}],
            "conversation_projects":[]
        });
        let receipt =
            import_chatgpt_history(&mut store, &serde_json::to_vec(&body).unwrap()).unwrap();
        assert_eq!(
            receipt.imported + receipt.updated + receipt.unchanged + receipt.failed,
            0
        );
        let projects = store.projects_snapshot("chatgpt").unwrap();
        assert_eq!(projects.projects[0]["title"], "Current sidebar name");
        assert_eq!(projects.projects[0]["conversation_count"], 1);
    }

    #[test]
    fn metadata_only_uncommitted_target_is_ignored_while_valid_mapping_is_applied() {
        let mut store = crate::store::Store::in_memory().unwrap();
        seed(&mut store, "existing");
        let scope = format!("chatgpt:observed:{}", "a".repeat(64));
        let body = serde_json::json!({
            "format":"chat-hub-history-v1", "account_scope":scope, "conversations":[],
            "project_catalog":[{"id":"project-1","title":"Project title"}],
            "conversation_projects":[{"conversation_id":"existing","project_id":"project-1"},{"conversation_id":"missing","project_id":"project-1"}]
        });
        let receipt =
            import_chatgpt_history(&mut store, &serde_json::to_vec(&body).unwrap()).unwrap();
        assert_eq!(receipt.failed, 0);
        let projects = store.projects_snapshot("chatgpt").unwrap();
        assert_eq!(projects.projects.len(), 1);
        assert_eq!(projects.projects[0]["title"], "Project title");
        assert_eq!(projects.projects[0]["conversation_count"], 1);
    }

    #[test]
    fn metadata_only_uncommitted_target_never_rebinds_another_account_row() {
        let mut store = crate::store::Store::in_memory().unwrap();
        seed(&mut store, "same-source-id");
        let other_scope = format!("chatgpt:observed:{}", "a".repeat(64));
        store
            .apply_project_memberships(
                "chatgpt",
                &other_scope,
                &[crate::store::ProjectMembership {
                    source_id: "same-source-id".into(),
                    project_id: Some("existing-project".into()),
                    title: Some("Other account project".into()),
                }],
            )
            .unwrap();
        let current_scope = format!("chatgpt:observed:{}", "b".repeat(64));
        let body = serde_json::json!({
            "format":"chat-hub-history-v1", "account_scope":current_scope, "conversations":[],
            "project_catalog":[{"id":"new-project","title":"Current account project"}],
            "conversation_projects":[{"conversation_id":"same-source-id","project_id":"new-project"}]
        });
        import_chatgpt_history(&mut store, &serde_json::to_vec(&body).unwrap()).unwrap();
        let projects = store.projects_snapshot("chatgpt").unwrap();
        assert_eq!(projects.projects.len(), 1);
        assert_eq!(projects.projects[0]["title"], "Other account project");
        assert_eq!(projects.projects[0]["conversation_count"], 1);
    }

    #[test]
    fn project_metadata_rejects_wrong_shapes_missing_id_and_bad_titles() {
        let mut store = crate::store::Store::in_memory().unwrap();
        let scope = format!("chatgpt:observed:{}", "a".repeat(64));
        let base = serde_json::json!({"format":"chat-hub-history-v1","account_scope":scope,"conversations":[]});
        for (field, value) in [
            ("project_catalog", serde_json::json!(true)),
            ("conversation_projects", serde_json::json!("bad")),
            (
                "project_catalog",
                serde_json::json!([{"id":"p","title":17}]),
            ),
            (
                "conversation_projects",
                serde_json::json!([{"conversation_id":"c"}]),
            ),
        ] {
            let mut envelope = base.clone();
            envelope[field] = value;
            assert!(
                import_chatgpt_history(&mut store, &serde_json::to_vec(&envelope).unwrap())
                    .is_err()
            );
        }
    }
}
