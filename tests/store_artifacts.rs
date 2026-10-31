mod common;

use chat_hub::{
    archive::prepare_restore,
    artifacts::{content_sha256, valid_filename, MAX_ARTIFACT_BYTES},
    catalog::CatalogEntityKind,
    store::{GraphNode, Observation, Store},
};
use rusqlite::Connection;
use serde_json::{json, Value};

fn source() -> Observation {
    Observation {
        provider: "chatgpt".into(),
        account_scope: "artifact-test-account".into(),
        source_id: "artifact-source".into(),
        title: "File source chat".into(),
        url: None,
        current_node: Some("assistant-file".into()),
        graph: vec![GraphNode {
            source_node_id: "assistant-file".into(),
            source_message_id: Some("source-message".into()),
            parent_id: None,
            role: Some("assistant".into()),
            author: None,
            create_time: None,
            parts: vec![Value::String("Created sandbox:/mnt/data/report.csv".into())],
        }],
        raw_payload: json!({"fixture":true}),
        source_time: None,
        observed_ms: 10,
        parser: "test".into(),
        completeness: "complete".into(),
    }
}

fn seeded() -> (Store, String, String) {
    let mut store = Store::in_memory().unwrap();
    let conversation_id = store.ingest_observation(&source()).unwrap();
    store.rebuild_catalog_index().unwrap();
    let entity = store
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
        .remove(0);
    (store, conversation_id, entity.id)
}

#[test]
fn generated_file_links_beyond_the_old_preview_budget_are_indexed() {
    let mut observation = source();
    observation.graph[0].parts = vec![Value::String(format!(
        "{}\nCreated sandbox:/mnt/data/late-report.csv",
        "prior discussion ".repeat(50_000)
    ))];
    let mut store = Store::in_memory().unwrap();
    let conversation_id = store.ingest_observation(&observation).unwrap();
    store.rebuild_catalog_index().unwrap();
    let page = store
        .catalog_entities(
            "chatgpt",
            None,
            Some(&conversation_id),
            Some(CatalogEntityKind::FileArtifact),
            None,
            None,
            10,
        )
        .unwrap();
    assert!(page
        .items
        .iter()
        .any(|item| item.filename.as_deref() == Some("late-report.csv")));
    assert!(!page.coverage_truncated);
}

#[test]
fn filename_validation_rejects_traversal_and_platform_path_forms() {
    for name in [
        "../report.csv",
        "..\\report.csv",
        "/tmp/report.csv",
        "C:report.csv",
        "report.csv:stream",
        "..",
        ".",
        "bad\nname",
        "trailing.",
        "trailing ",
    ] {
        assert!(!valid_filename(name), "accepted unsafe filename {name:?}");
    }
    assert!(valid_filename("report-final.csv"));
    assert!(valid_filename("résumé 2026.csv"));
}

#[test]
fn save_is_bound_to_source_conversation_and_reports_verified_sha() {
    let (mut store, conversation_id, entity_id) = seeded();
    assert!(store
        .save_artifact_content(
            &entity_id,
            "another-conversation",
            "report.csv",
            Some("text/csv"),
            b"a,b\n1,2\n"
        )
        .is_err());
    assert!(store
        .save_artifact_content(
            "entity:not-a-file",
            &conversation_id,
            "report.csv",
            None,
            b"bytes"
        )
        .is_err());
    for name in ["../escape", "C:escape", "name/"] {
        assert!(store
            .save_artifact_content(&entity_id, &conversation_id, name, None, b"bytes")
            .is_err());
    }
    let bytes = b"a,b\n1,2\n";
    let saved = store
        .save_artifact_content(
            &entity_id,
            &conversation_id,
            "report.csv",
            Some("text/csv"),
            bytes,
        )
        .unwrap();
    assert_eq!(saved.byte_size, bytes.len() as i64);
    assert_eq!(saved.sha256, content_sha256(bytes));
    assert_eq!(
        store.artifact_content(&entity_id).unwrap().unwrap().1,
        bytes
    );
    let projected = store.catalog_entity(&entity_id).unwrap().unwrap().entity;
    assert!(projected.content_saved);
    assert_eq!(projected.artifact_state.as_deref(), Some("saved_locally"));
}

#[test]
fn artifact_reader_rejects_database_bytes_with_a_mismatched_sha() {
    let dir = common::temp_dir("artifact-integrity");
    let path = dir.join("library.sqlite");
    let (conversation_id, entity_id);
    {
        let mut store = Store::open(path.to_str().unwrap()).unwrap();
        conversation_id = store.ingest_observation(&source()).unwrap();
        store.rebuild_catalog_index().unwrap();
        entity_id = store
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
        store
            .save_artifact_content(
                &entity_id,
                &conversation_id,
                "report.csv",
                None,
                b"expected bytes",
            )
            .unwrap();
    }
    Connection::open(&path)
        .unwrap()
        .execute(
            "UPDATE artifact_contents SET content=x'00',byte_size=1 WHERE entity_id=?1",
            [&entity_id],
        )
        .unwrap();
    let store = Store::open_existing(path.to_str().unwrap()).unwrap();
    assert!(
        store.artifact_content(&entity_id).is_err(),
        "integrity mismatch must never return bytes"
    );
    drop(store);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn artifact_source_membership_check_is_not_limited_by_detail_reference_page() {
    let mut store = Store::in_memory().unwrap();
    let mut conversations = Vec::new();
    for index in 0..101 {
        let source_id = format!("source-{index:03}");
        conversations.push(store.ingest_observation(&Observation {
            provider:"chatgpt".into(),account_scope:"shared-file-account".into(),source_id:source_id.clone(),
            title:source_id.clone(),url:None,current_node:Some("file-node".into()),
            graph:vec![GraphNode { source_node_id:"file-node".into(),source_message_id:Some(format!("message-{index}")),
                parent_id:None,role:Some("assistant".into()),author:None,create_time:None,
                parts:vec![json!({"filename":"shared.csv","file_id":"stable-provider-file-id","mime_type":"text/csv"})] }],
            raw_payload:json!({}),source_time:None,observed_ms:index,parser:"test".into(),completeness:"complete".into(),
        }).unwrap());
    }
    store.rebuild_catalog_index().unwrap();
    let entity = store
        .catalog_entities(
            "chatgpt",
            None,
            None,
            Some(CatalogEntityKind::FileArtifact),
            None,
            None,
            10,
        )
        .unwrap()
        .items
        .remove(0);
    let last = conversations.last().unwrap();
    let detail = store.catalog_entity(&entity.id).unwrap().unwrap();
    assert!(detail.references_truncated);
    assert!(!detail
        .references
        .iter()
        .any(|reference| &reference.conversation_id == last));
    assert!(store
        .save_artifact_content(
            &entity.id,
            last,
            "shared.csv",
            Some("text/csv"),
            b"owned by explicit source chat"
        )
        .is_ok());
}

#[test]
fn catalog_entity_tag_filter_precedes_count_and_keyset_page_limit() {
    let mut store = Store::in_memory().unwrap();
    let first = store.ingest_observation(&source()).unwrap();
    let mut second_source = source();
    second_source.source_id = "artifact-source-second".into();
    second_source.title = "Second file source".into();
    let second = store.ingest_observation(&second_source).unwrap();
    store.rebuild_catalog_index().unwrap();
    let page = store
        .catalog_entities(
            "chatgpt",
            None,
            None,
            Some(CatalogEntityKind::FileArtifact),
            None,
            None,
            10,
        )
        .unwrap();
    assert_eq!(page.items.len(), 2);
    let tagged_entity = page.items.iter().max_by(|a, b| a.id.cmp(&b.id)).unwrap();
    let tagged_id = tagged_entity.id.clone();
    store
        .set_target_tags("file", &tagged_id, &["Work Item".into()])
        .unwrap();
    let filtered = store
        .catalog_entities_tagged(
            "chatgpt",
            None,
            None,
            Some(CatalogEntityKind::FileArtifact),
            None,
            Some(" work   item "),
            None,
            1,
        )
        .unwrap();
    assert_eq!(filtered.total, 1);
    assert_eq!(filtered.items.len(), 1);
    assert_eq!(filtered.items[0].id, tagged_id);
    assert_eq!(filtered.tag.as_deref(), Some(" work   item "));
    assert_ne!(
        filtered.items[0].id,
        page.items.iter().find(|e| e.id != tagged_id).unwrap().id
    );
    let _ = (first, second);
}

#[test]
fn byte_cap_is_enforced_before_storage() {
    let (mut store, conversation_id, entity_id) = seeded();
    let oversized = vec![0_u8; MAX_ARTIFACT_BYTES + 1];
    assert!(store
        .save_artifact_content(&entity_id, &conversation_id, "report.csv", None, &oversized)
        .is_err());
    assert!(store.saved_artifact(&entity_id).unwrap().is_none());
}

#[test]
fn saved_bytes_survive_catalog_rebuild_and_sqlite_backup_restore() {
    let dir = common::temp_dir("artifact-backup-restore");
    let original_path = dir.join("source.sqlite");
    let backup_path = dir.join("backup.sqlite");
    let restored_path = dir.join("restored.sqlite");
    let bytes = b"locally selected generated file\0with binary bytes";
    let (conversation_id, entity_id);
    {
        let mut store = Store::open(original_path.to_str().unwrap()).unwrap();
        conversation_id = store.ingest_observation(&source()).unwrap();
        store.rebuild_catalog_index().unwrap();
        entity_id = store
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
        store
            .save_artifact_content(
                &entity_id,
                &conversation_id,
                "report.csv",
                Some("text/csv"),
                bytes,
            )
            .unwrap();
        store.rebuild_catalog_index().unwrap();
        assert_eq!(
            store.artifact_content(&entity_id).unwrap().unwrap().1,
            bytes
        );
        store.backup_to(&backup_path).unwrap();
    }
    drop(Store::open(restored_path.to_str().unwrap()).unwrap());
    let prepared = prepare_restore(&backup_path, &restored_path).unwrap();
    prepared.commit().unwrap();
    let restored = Store::open_existing(restored_path.to_str().unwrap()).unwrap();
    let (metadata, restored_bytes) = restored.artifact_content(&entity_id).unwrap().unwrap();
    assert_eq!(restored_bytes, bytes);
    assert_eq!(metadata.conversation_id, conversation_id);
    assert_eq!(metadata.sha256, content_sha256(bytes));
    assert_eq!(
        restored
            .saved_artifacts(Some(&conversation_id), None, 10)
            .unwrap()
            .len(),
        1
    );
    drop(restored);
    std::fs::remove_dir_all(dir).unwrap();
}
