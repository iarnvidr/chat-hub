mod common;

use chat_hub::store::{GraphNode, Observation, PageScope, Store};

fn observation() -> Observation {
    Observation {
        provider: "chatgpt".into(),
        account_scope: "chatgpt:observed:fixture".into(),
        source_id: "conversation-source-1".into(),
        title: "Tagged chat".into(),
        url: None,
        current_node: None,
        graph: vec![GraphNode {
            source_node_id: "root".into(),
            source_message_id: Some("message-1".into()),
            parent_id: None,
            role: Some("user".into()),
            author: None,
            create_time: None,
            parts: vec![],
        }],
        raw_payload: serde_json::json!({}),
        source_time: None,
        observed_ms: 1,
        parser: "fixture".into(),
        completeness: "complete".into(),
    }
}

#[test]
fn tags_persist_across_open_and_v12_migration_isolated_from_index() {
    let db = common::temp_dir("store-tags").join("library.sqlite");
    let path = db.to_str().unwrap();
    let mut store = Store::open(path).unwrap();
    assert_eq!(store.schema_version().unwrap(), 13);
    let conversation_id = store.ingest_observation(&observation()).unwrap();
    store
        .set_target_tags(
            "conversation",
            &conversation_id,
            &["Planning".into(), "Reference".into()],
        )
        .unwrap();
    store
        .set_target_tags("file", "entity:file-digest", &["Reference".into()])
        .unwrap();
    drop(store);

    let store = Store::open(path).unwrap();
    assert_eq!(store.schema_version().unwrap(), 13);
    assert_eq!(
        store.target_tags("conversation", &conversation_id).unwrap(),
        ["Planning", "Reference"]
    );
    assert_eq!(
        store.target_tags("file", "entity:file-digest").unwrap(),
        ["Reference"]
    );
    drop(store);

    let conn = rusqlite::Connection::open(path).unwrap();
    conn.execute_batch("DROP TRIGGER target_tags_conversation_cleanup; DROP INDEX target_tags_by_tag; DROP INDEX target_tags_by_target; DROP INDEX artifact_contents_by_conversation; DROP TABLE artifact_contents; DROP TABLE target_tags; DROP TABLE tags; DROP TABLE tag_state; DROP INDEX conversation_local_state_page; DROP TABLE conversation_local_state; DELETE FROM schema_migrations WHERE version>=12;")
        .unwrap();
    drop(conn);
    let migrated = Store::open(path).unwrap();
    assert_eq!(migrated.schema_version().unwrap(), 13);
    assert!(migrated
        .target_tags("conversation", &conversation_id)
        .unwrap()
        .is_empty());
}

#[test]
fn tagged_list_and_search_filter_before_paging_and_capture_tag_revision() {
    let mut store = Store::in_memory().unwrap();
    let first = store.ingest_observation(&observation()).unwrap();
    let mut second_observation = observation();
    second_observation.source_id = "conversation-source-2".into();
    second_observation.title = "Different topic".into();
    let second = store.ingest_observation(&second_observation).unwrap();
    store
        .set_target_tags("conversation", &first, &["Planning".into()])
        .unwrap();
    store
        .set_target_tags("conversation", &second, &["Reference".into()])
        .unwrap();

    let all = store
        .source_list_page_snapshot_with_tag(10, None, None, "chatgpt", None, None)
        .unwrap();
    assert_eq!(all.items.len(), 2);
    assert_eq!(all.tag_revision, None);

    let planning = store
        .source_list_page_snapshot_with_tag(1, None, None, "chatgpt", None, Some("planning"))
        .unwrap();
    assert_eq!(planning.items.len(), 1);
    assert_eq!(planning.items[0]["id"], first);
    let planning_revision = planning.tag_revision.unwrap();

    let search = store
        .source_search_page_snapshot_with_tag(
            "Tagged",
            10,
            None,
            None,
            PageScope {
                source: "chatgpt",
                project_key: None,
                tag: Some("Planning"),
            },
        )
        .unwrap();
    assert_eq!(search.hits.len(), 1);
    assert_eq!(search.hits[0]["conversation"]["id"], first);
    assert_eq!(search.tag_revision, Some(planning_revision));

    store
        .set_target_tags("conversation", &second, &["Planning".into()])
        .unwrap();
    let after_edit = store
        .source_list_page_snapshot_with_tag(1, None, None, "chatgpt", None, Some("Planning"))
        .unwrap();
    assert!(after_edit.tag_revision.unwrap() > planning_revision);
    assert_eq!(after_edit.items.len(), 1);
}
