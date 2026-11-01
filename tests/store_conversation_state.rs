use chat_hub::store::{
    ConversationStateChange, GraphNode, Observation, ProjectMembership, Store,
    VerifiedConversationStateReceipt, NO_PROJECT_FILTER,
};
use serde_json::Value;

fn seed_legacy_chat(store: &mut Store) -> String {
    store
        .ingest_observation(&Observation {
            provider: "paseo-chatgpt".into(),
            account_scope: "paseo-cache:owner".into(),
            source_id: "chat-uuid-1".into(),
            title: "Saved chat".into(),
            url: None,
            current_node: None,
            graph: vec![],
            raw_payload: Value::Null,
            source_time: None,
            observed_ms: 1,
            parser: "test".into(),
            completeness: "complete".into(),
        })
        .unwrap()
}

fn receipt(conversation_id: &str) -> VerifiedConversationStateReceipt {
    VerifiedConversationStateReceipt {
        verified: true,
        conversation_id: conversation_id.into(),
        source_conversation_id: "chat-uuid-1".into(),
        scope: format!("chatgpt:observed:{}", "a".repeat(64)),
        target_provider: "paseo-chatgpt".into(),
        target_scope: "paseo-cache:owner".into(),
        archived: Some(true),
        pinned: Some(false),
    }
}

fn seed_chat(store: &mut Store, source_id: &str, updated: &str) -> String {
    store
        .ingest_observation(&Observation {
            provider: "chatgpt".into(),
            account_scope: format!("chatgpt:observed:{}", "b".repeat(64)),
            source_id: source_id.into(),
            title: format!("needle {source_id}"),
            url: Some(format!("https://chatgpt.com/c/{source_id}")),
            current_node: Some("node-1".into()),
            graph: vec![GraphNode {
                source_node_id: "node-1".into(),
                source_message_id: Some(format!("{source_id}-message")),
                parent_id: None,
                role: Some("user".into()),
                author: None,
                create_time: None,
                parts: vec![serde_json::json!(format!(
                    "needle https://github.com/acme/{source_id}"
                ))],
            }],
            raw_payload: Value::Null,
            source_time: Some(updated.into()),
            observed_ms: 10,
            parser: "test".into(),
            completeness: "complete".into(),
        })
        .unwrap()
}

fn apply_verified_state(
    store: &mut Store,
    conversation_id: &str,
    source_id: &str,
    pinned: Option<bool>,
    archived: Option<bool>,
) {
    let scope = format!("chatgpt:observed:{}", "b".repeat(64));
    let receipt = VerifiedConversationStateReceipt {
        verified: true,
        conversation_id: conversation_id.into(),
        source_conversation_id: source_id.into(),
        scope: scope.clone(),
        target_provider: "chatgpt".into(),
        target_scope: scope,
        archived,
        pinned,
    };
    store
        .commit_verified_conversation_state(
            conversation_id,
            &receipt,
            ConversationStateChange { archived, pinned },
        )
        .unwrap();
}

#[test]
fn state_commit_requires_exact_verified_row_binding_and_readback() {
    let mut store = Store::in_memory().unwrap();
    let id = seed_legacy_chat(&mut store);
    let initial_revision = store.library_revision().unwrap();

    let mut invalid = receipt(&id);
    invalid.target_scope = "paseo-cache:other".into();
    assert!(store
        .commit_verified_conversation_state(
            &id,
            &invalid,
            ConversationStateChange {
                archived: Some(true),
                pinned: None,
            },
        )
        .is_err());
    assert_eq!(store.library_revision().unwrap(), initial_revision);

    let valid = receipt(&id);
    assert!(store
        .commit_verified_conversation_state(
            &id,
            &valid,
            ConversationStateChange {
                archived: Some(false),
                pinned: None,
            },
        )
        .is_err());
    assert_eq!(store.library_revision().unwrap(), initial_revision);

    let committed = store
        .commit_verified_conversation_state(
            &id,
            &valid,
            ConversationStateChange {
                archived: Some(true),
                pinned: None,
            },
        )
        .unwrap();
    assert!(committed.archived);
    assert!(!committed.pinned);
    assert_eq!(committed.library_revision, initial_revision + 1);

    let unchanged = store
        .commit_verified_conversation_state(
            &id,
            &valid,
            ConversationStateChange {
                archived: Some(true),
                pinned: None,
            },
        )
        .unwrap();
    assert_eq!(unchanged.library_revision, committed.library_revision);
}

#[test]
fn pinned_state_also_requires_provider_readback_and_legacy_scope_binding() {
    let mut store = Store::in_memory().unwrap();
    let id = seed_legacy_chat(&mut store);
    let mut missing_readback = receipt(&id);
    missing_readback.pinned = None;
    assert!(store
        .commit_verified_conversation_state(
            &id,
            &missing_readback,
            ConversationStateChange {
                archived: None,
                pinned: Some(true),
            },
        )
        .is_err());

    let verified = receipt(&id);
    let committed = store
        .commit_verified_conversation_state(
            &id,
            &verified,
            ConversationStateChange {
                archived: None,
                pinned: Some(false),
            },
        )
        .unwrap();
    assert!(!committed.pinned);
    assert!(!committed.archived);
}

#[test]
fn no_project_active_archived_and_pinned_filters_apply_before_pages_and_catalog_counts() {
    let mut store = Store::in_memory().unwrap();
    let assigned = seed_chat(&mut store, "assigned-source", "2026-01-03T00:00:00Z");
    let archived = seed_chat(&mut store, "archived-source", "2026-01-02T00:00:00Z");
    let pinned = seed_chat(&mut store, "pinned-source", "2025-01-01T00:00:00Z");
    assert!(store
        .rename_project(NO_PROJECT_FILTER, Some("collision"))
        .is_err());
    assert!(store
        .commit_project_assignment(
            &assigned,
            &format!("chatgpt:observed:{}", "b".repeat(64)),
            Some(NO_PROJECT_FILTER),
            None,
        )
        .is_err());
    store
        .apply_project_memberships(
            "chatgpt",
            &format!("chatgpt:observed:{}", "b".repeat(64)),
            &[ProjectMembership {
                source_id: "assigned-source".into(),
                project_id: Some("g-p-project-1".into()),
                title: Some("Project".into()),
            }],
        )
        .unwrap();
    apply_verified_state(&mut store, &archived, "archived-source", None, Some(true));
    apply_verified_state(&mut store, &pinned, "pinned-source", Some(true), None);
    store.rebuild_catalog_index().unwrap();

    let active = store
        .source_list_page_snapshot_with_tag_and_archive_state(
            1, None, None, "chatgpt", None, None, "active",
        )
        .unwrap();
    assert_eq!(active.items[0]["id"], pinned);
    assert_eq!(active.items[0]["pinned"], true);
    let second = store
        .source_list_page_snapshot_with_tag_and_archive_state(
            2,
            Some(&chat_hub::store::ListKey {
                pinned: true,
                updated_at_ms: active.items[0]["updated_at_ms"].as_i64(),
                id: pinned.clone(),
            }),
            Some(active.library_revision),
            "chatgpt",
            None,
            None,
            "active",
        )
        .unwrap();
    assert_eq!(second.items.len(), 1);
    assert_eq!(second.items[0]["id"], assigned);

    let no_project_active = store
        .source_list_page_snapshot_with_tag_and_archive_state(
            1,
            None,
            None,
            "chatgpt",
            Some(NO_PROJECT_FILTER),
            None,
            "active",
        )
        .unwrap();
    assert_eq!(no_project_active.items.len(), 1);
    assert_eq!(no_project_active.items[0]["id"], pinned);
    let no_project_archived = store
        .source_list_page_snapshot_with_tag_and_archive_state(
            1,
            None,
            None,
            "chatgpt",
            Some(NO_PROJECT_FILTER),
            None,
            "archived",
        )
        .unwrap();
    assert_eq!(no_project_archived.items.len(), 1);
    assert_eq!(no_project_archived.items[0]["id"], archived);

    let active_search = store
        .source_search_page_snapshot_with_tag_and_archive_state(
            "needle",
            1,
            None,
            None,
            chat_hub::store::PageScope {
                source: "chatgpt",
                project_key: Some(NO_PROJECT_FILTER),
                tag: None,
            },
            "active",
        )
        .unwrap();
    assert!(!active_search.hits.is_empty());
    assert!(active_search
        .hits
        .iter()
        .all(|hit| hit["conversation"]["id"] == pinned));
    let archived_search = store
        .source_search_page_snapshot_with_tag_and_archive_state(
            "needle",
            1,
            None,
            None,
            chat_hub::store::PageScope {
                source: "chatgpt",
                project_key: Some(NO_PROJECT_FILTER),
                tag: None,
            },
            "archived",
        )
        .unwrap();
    assert!(!archived_search.hits.is_empty());
    assert!(archived_search
        .hits
        .iter()
        .all(|hit| hit["conversation"]["id"] == archived));

    let overview = store
        .catalog_overview("chatgpt", Some(NO_PROJECT_FILTER))
        .unwrap();
    assert_eq!(overview.stats.chats, 2);
    let graph = store
        .catalog_graph("chatgpt", Some(NO_PROJECT_FILTER), 20)
        .unwrap();
    assert_eq!(graph.stats.chats, 2);
    assert_eq!(graph.unmapped_count, 0);
    assert_eq!(graph.nodes.len(), 2);
    let entities = store
        .catalog_entities_search(chat_hub::catalog::CatalogEntityQuery {
            source: "chatgpt",
            project_key: Some(NO_PROJECT_FILTER),
            conversation_id: None,
            kind: None,
            artifact_state: None,
            tag: None,
            query: None,
            cursor: None,
            limit: 100,
        })
        .unwrap();
    assert!(entities.items.len() >= 2);
    assert_eq!(entities.total, entities.items.len() as i64);
}
