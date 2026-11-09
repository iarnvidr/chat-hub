//! Exercise recovery through the real ingestion adapter and a file-backed Store.
use chat_hub_model::{Observation, ObservationSink};
use chat_hub_store::{capture::normalize_chatgpt_document_in_scope, store::Store};
use ring::rand::SecureRandom;
use rusqlite::Connection;
use serde_json::json;
use std::{fs, path::PathBuf};

const SOURCE: &str = "123e4567-e89b-12d3-a456-426614174000";

struct Fixture {
    directory: PathBuf,
    database: String,
}

impl Fixture {
    fn new() -> Self {
        let mut random = [0u8; 16];
        ring::rand::SystemRandom::new().fill(&mut random).unwrap();
        let suffix: String = random.iter().map(|byte| format!("{byte:02x}")).collect();
        let directory = std::env::temp_dir().join(format!("chat-hub-recovery-{suffix}"));
        fs::create_dir(&directory).unwrap();
        let database = directory
            .join("library.sqlite3")
            .to_str()
            .unwrap()
            .to_owned();
        Self {
            directory,
            database,
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.directory);
    }
}

fn transcript(scope: &str, updated: i64, reply: &str) -> Observation {
    normalize_chatgpt_document_in_scope(
        &json!({
            "conversation_id": SOURCE,
            "title": "Synthetic restart acceptance",
            "update_time": updated,
            "current_node": "reply",
            "mapping": {
                "prompt": {
                    "parent": null,
                    "message": {
                        "id": "prompt",
                        "author": {"role": "user"},
                        "content": {"content_type": "text", "parts": ["Durable prompt marker"]}
                    }
                },
                "reply": {
                    "parent": "prompt",
                    "message": {
                        "id": "reply",
                        "author": {"role": "assistant"},
                        "content": {"content_type": "text", "parts": [reply]}
                    }
                }
            }
        }),
        "chatgpt-live-v1",
        updated * 1000,
        scope,
    )
    .unwrap()
}

#[test]
fn failed_transcript_commit_recovers_prompt_reply_and_search_after_restart() {
    let fixture = Fixture::new();
    let scope = format!("chatgpt:observed:{}", "a".repeat(64));
    let other_scope = format!("chatgpt:observed:{}", "b".repeat(64));
    let mut store = Store::open_service(&fixture.database).unwrap();
    let fault = Connection::open(&fixture.database).unwrap();
    fault
        .execute_batch(
            "CREATE TRIGGER fail_capture BEFORE INSERT ON observations BEGIN
             SELECT RAISE(ABORT, 'synthetic commit failure'); END;",
        )
        .unwrap();
    assert!(store
        .store_observation(&transcript(&scope, 1, "Durable reply marker"))
        .is_err());
    assert!(!store.contains_source("chatgpt", &scope, SOURCE).unwrap());
    assert_eq!(
        Store::capture_recovery_status(&fixture.database)
            .unwrap()
            .pending_items,
        1
    );
    drop(store);
    fault.execute_batch("DROP TRIGGER fail_capture").unwrap();
    drop(fault);

    let mut reopened = Store::open_service(&fixture.database).unwrap();
    let receipt = reopened.recover_pending_captures().unwrap();
    assert_eq!(receipt.recovered, 1);
    assert_eq!(receipt.failed, 0);
    assert_eq!(receipt.pending_items, 0);
    assert!(reopened.contains_source("chatgpt", &scope, SOURCE).unwrap());
    assert!(!reopened
        .contains_source("chatgpt", &other_scope, SOURCE)
        .unwrap());
    let saved = reopened.read(SOURCE).unwrap().unwrap();
    let messages = saved["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 2);
    assert!(messages
        .iter()
        .any(|message| message["content"] == "Durable prompt marker"));
    assert!(messages
        .iter()
        .any(|message| message["content"] == "Durable reply marker"));
    assert_eq!(saved["current_node"], "reply");
    assert_eq!(
        reopened.search("Durable reply marker", 10).unwrap().len(),
        1
    );
    assert_eq!(reopened.recover_pending_captures().unwrap().recovered, 0);
    assert_eq!(reopened.library_snapshot().unwrap().conversations.len(), 1);
}

#[test]
fn delayed_recovery_does_not_replace_a_newer_committed_snapshot() {
    let fixture = Fixture::new();
    let scope = format!("chatgpt:observed:{}", "a".repeat(64));
    let mut store = Store::open_service(&fixture.database).unwrap();
    let fault = Connection::open(&fixture.database).unwrap();
    fault
        .execute_batch(
            "CREATE TRIGGER fail_capture BEFORE INSERT ON observations BEGIN
             SELECT RAISE(ABORT, 'synthetic commit failure'); END;",
        )
        .unwrap();
    assert!(store
        .store_observation(&transcript(&scope, 1, "Older reply"))
        .is_err());
    fault.execute_batch("DROP TRIGGER fail_capture").unwrap();
    drop(fault);
    // A history import can independently commit a newer snapshot before replay.
    store
        .ingest_observation(&transcript(&scope, 2, "Newer reply"))
        .unwrap();
    drop(store);
    let mut reopened = Store::open_service(&fixture.database).unwrap();
    assert_eq!(reopened.recover_pending_captures().unwrap().recovered, 1);
    let saved = reopened.read(SOURCE).unwrap().unwrap();
    let messages = saved["messages"].as_array().unwrap();
    assert!(messages
        .iter()
        .any(|message| message["content"] == "Newer reply"));
    assert!(!messages
        .iter()
        .any(|message| message["content"] == "Older reply"));
    assert_eq!(reopened.library_snapshot().unwrap().conversations.len(), 1);
}
