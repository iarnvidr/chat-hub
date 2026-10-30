use chat_hub::importer::{parse_export, read_input, MAX_BYTES, MAX_ENTRIES};
use chat_hub::{importer::import_chatgpt, store::Store};
use serde_json::{json, Value};

const ARRAY: &[u8] = include_bytes!("fixtures/chatgpt-export/array.json");
const OBJECT: &[u8] = include_bytes!("fixtures/chatgpt-export/object.json");

fn temporary_dir() -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!(
        "chat-hub-import-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&path).unwrap();
    path
}

#[test]
fn durable_export_graph_reimport_and_updates() {
    let dir = temporary_dir();
    let path = dir.join("library.sqlite");
    let mut store = Store::open(path.to_str().unwrap()).unwrap();
    assert_eq!(import_chatgpt(&mut store, ARRAY).unwrap().imported, 1);
    assert_eq!(import_chatgpt(&mut store, OBJECT).unwrap().imported, 1);
    let revision = store.library_revision().unwrap();
    let snapshot = store.library_snapshot().unwrap();
    let id = snapshot
        .conversations
        .iter()
        .find(|c| c["source_id"] == "synthetic-export-one")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let graph = store.read_normalized(&id).unwrap().unwrap();
    assert_eq!(graph["nodes"].as_array().unwrap().len(), 3);
    assert_eq!(graph["messages"].as_array().unwrap().len(), 2);
    assert_eq!(graph["selected_node_id"], "right");
    let root = graph["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["id"] == "root")
        .unwrap();
    assert_eq!(root["message_id"], Value::Null);
    let right = graph["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["node_id"] == "right")
        .unwrap();
    assert_eq!(right["parts"][1]["kind"], "unknown");
    assert_eq!(
        right["parts"][1]["raw"],
        json!({"content_type":"future_part","payload":{"nested":[1,true,null]}})
    );
    assert_eq!(import_chatgpt(&mut store, ARRAY).unwrap().unchanged, 1);
    assert_eq!(import_chatgpt(&mut store, OBJECT).unwrap().unchanged, 1);
    assert_eq!(store.library_revision().unwrap(), revision);
    assert_eq!(store.read_normalized(&id).unwrap().unwrap(), graph);
    drop(store);
    let mut store = Store::open(path.to_str().unwrap()).unwrap();
    assert_eq!(store.library_revision().unwrap(), revision);
    assert_eq!(store.read_normalized(&id).unwrap().unwrap(), graph);
    let mut update: Value = serde_json::from_slice(ARRAY).unwrap();
    update[0]["title"] = json!("Updated synthetic title");
    update[0]["update_time"] = json!(1700000200);
    assert_eq!(
        import_chatgpt(&mut store, &serde_json::to_vec(&update).unwrap())
            .unwrap()
            .updated,
        1
    );
    assert!(store.library_revision().unwrap() > revision);
    drop(store);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn invalid_global_input_never_commits_and_item_failures_continue() {
    let mut store = Store::in_memory().unwrap();
    let before = store.library_revision().unwrap();
    let mut malformed = ARRAY.to_vec();
    malformed.extend_from_slice(b"trailing secret marker");
    assert!(import_chatgpt(&mut store, &malformed).is_err());
    assert_eq!(store.library_revision().unwrap(), before);
    assert!(store.library_snapshot().unwrap().conversations.is_empty());
    for bytes in [
        b"\xff".as_slice(),
        b"{}",
        b"null",
        b"{\"conversations\":null}",
    ] {
        assert!(import_chatgpt(&mut store, bytes).is_err());
        assert_eq!(store.library_revision().unwrap(), before);
    }
    let hundred = serde_json::to_vec(&vec![Value::Null; 100]).unwrap();
    let receipt = import_chatgpt(&mut store, &hundred).unwrap();
    assert_eq!(receipt.failed, 100);
    assert_eq!(receipt.errors.len(), 100);
    assert!(!receipt.errors_truncated);
    assert_eq!(store.library_revision().unwrap(), before);
    let valid = parse_export(ARRAY).unwrap().remove(0);
    let mut entries = vec![Value::Null; 102];
    entries.insert(1, valid);
    let receipt = import_chatgpt(&mut store, &serde_json::to_vec(&entries).unwrap()).unwrap();
    assert_eq!(receipt.imported, 1);
    assert_eq!(receipt.failed, 102);
    assert_eq!(receipt.errors.len(), 100);
    assert!(receipt.errors_truncated);
    assert_eq!(receipt.errors[0].item_index, 0);
    assert_eq!(receipt.errors[1].item_index, 2);
    assert_eq!(
        receipt.imported + receipt.updated + receipt.unchanged + receipt.failed,
        entries.len()
    );
    let serialized = serde_json::to_value(&receipt).unwrap();
    assert_eq!(serialized.as_object().unwrap().len(), 6);
    for error in receipt.errors {
        assert_eq!(error.code, "invalid_item");
        assert_eq!(error.message, "Conversation entry is invalid.");
    }
}

#[test]
fn duplicate_items_are_classified_after_each_commit() {
    let mut store = Store::in_memory().unwrap();
    let doc = parse_export(ARRAY).unwrap().remove(0);
    let input = serde_json::to_vec(&vec![doc.clone(), doc]).unwrap();
    let receipt = import_chatgpt(&mut store, &input).unwrap();
    assert_eq!(
        (
            receipt.imported,
            receipt.updated,
            receipt.unchanged,
            receipt.failed
        ),
        (1, 0, 1, 0)
    );
    assert_eq!(store.library_snapshot().unwrap().conversations.len(), 1);
    let empty = import_chatgpt(&mut store, b"[]").unwrap();
    assert_eq!(empty, Default::default());
}

#[test]
fn store_write_abort_rolls_back_only_failed_item_and_redacts_diagnostics() {
    let dir = temporary_dir();
    let path = dir.join("library.sqlite");
    let mut store = Store::open(path.to_str().unwrap()).unwrap();
    let injector = rusqlite::Connection::open(&path).unwrap();
    // Abort after conversation insertion, while writing the observation, so the
    // failed item's already-written rows must roll back with its transaction.
    injector
        .execute_batch(
            "CREATE TRIGGER synthetic_import_abort BEFORE INSERT ON observations
        WHEN (SELECT source_id FROM conversations WHERE id=NEW.conversation_id)='fail-item'
        BEGIN SELECT RAISE(ABORT, 'PRIVATE_SQL_SENTINEL'); END;",
        )
        .unwrap();
    let template = parse_export(ARRAY).unwrap().remove(0);
    let entries: Vec<Value> = ["before-item", "fail-item", "after-item"]
        .iter()
        .map(|id| {
            let mut doc = template.clone();
            doc["id"] = json!(id);
            doc
        })
        .collect();
    let receipt = import_chatgpt(&mut store, &serde_json::to_vec(&entries).unwrap()).unwrap();
    assert_eq!(
        (
            receipt.imported,
            receipt.updated,
            receipt.unchanged,
            receipt.failed
        ),
        (2, 0, 0, 1)
    );
    assert_eq!(receipt.errors.len(), 1);
    assert_eq!(receipt.errors[0].item_index, 1);
    assert_eq!(receipt.errors[0].code, "store_failed");
    assert!(!serde_json::to_string(&receipt)
        .unwrap()
        .contains("PRIVATE_SQL_SENTINEL"));
    assert!(!receipt.errors_truncated);
    drop(store);
    let store = Store::open(path.to_str().unwrap()).unwrap();
    let snapshot = store.library_snapshot().unwrap();
    let mut sources: Vec<_> = snapshot
        .conversations
        .iter()
        .map(|c| c["source_id"].as_str().unwrap())
        .collect();
    sources.sort();
    assert_eq!(sources, ["after-item", "before-item"]);
    let failed_rows: i64 = injector
        .query_row(
            "SELECT count(*) FROM conversations WHERE source_id='fail-item'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(failed_rows, 0);
    let observations: i64 = injector
        .query_row("SELECT count(*) FROM observations", [], |r| r.get(0))
        .unwrap();
    assert_eq!(observations, 2);
    drop(store);
    drop(injector);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn global_envelopes_and_empty_input() {
    assert_eq!(parse_export(b"[]").unwrap().len(), 0);
    assert_eq!(
        parse_export(br#"{"conversations":[{},null]}"#)
            .unwrap()
            .len(),
        2
    );
    for input in [
        b"null".as_slice(),
        b"{}",
        b"[{},",
        b"{\"conversations\":{}}",
        b"\xff",
    ] {
        let error = parse_export(input).unwrap_err();
        assert_eq!(error.code, "invalid_request");
        assert!(!error.message.contains("SQL"));
    }
    assert!(parse_export(b"PK\x03\x04")
        .unwrap_err()
        .message
        .contains("Extract conversations.json"));
}

#[test]
fn actual_bytes_and_entry_limits() {
    let mut input = vec![b' '; MAX_BYTES];
    input[0] = b'[';
    input[1] = b']';
    assert!(parse_export(&input).is_ok());
    input.push(b' ');
    assert_eq!(parse_export(&input).unwrap_err().code, "payload_too_large");
    assert_eq!(
        read_input(input.as_slice()).unwrap_err().code,
        "payload_too_large"
    );
    let entries = vec![serde_json::Value::Null; MAX_ENTRIES];
    assert_eq!(
        parse_export(&serde_json::to_vec(&entries).unwrap())
            .unwrap()
            .len(),
        MAX_ENTRIES
    );
    let entries = vec![serde_json::Value::Null; MAX_ENTRIES + 1];
    assert!(parse_export(&serde_json::to_vec(&entries).unwrap()).is_err());
}
