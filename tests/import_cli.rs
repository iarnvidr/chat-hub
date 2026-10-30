use chat_hub::store::Store;
use std::{fs, process::Command};

#[test]
fn cli_import_uses_canonical_config_and_persists_across_processes() {
    let dir = std::env::temp_dir().join(format!(
        "chat-hub-import-cli-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&dir).unwrap();
    let input = dir.join("conversations.json");
    fs::write(&input, include_bytes!("fixtures/chatgpt-export/array.json")).unwrap();
    for (count, expected) in [("imported", 1), ("unchanged", 1)] {
        let output = Command::new(env!("CARGO_BIN_EXE_chat-hub"))
            .arg("import")
            .arg(&input)
            .env("CHAT_HUB_TOKEN", "synthetic-test-token")
            .env("CHAT_HUB_DATA_DIR", &dir)
            .env_remove("CHAT_HUB_DATABASE")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let receipt: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(receipt[count], expected);
        assert_eq!(receipt["failed"], 0);
    }
    let store = Store::open(dir.join("chat-hub.sqlite3").to_str().unwrap()).unwrap();
    assert_eq!(store.library_snapshot().unwrap().conversations.len(), 1);
    drop(store);
    fs::write(&input, b"[invalid sensitive-marker").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_chat-hub"))
        .arg("import")
        .arg(&input)
        .env("CHAT_HUB_TOKEN", "synthetic-test-token")
        .env("CHAT_HUB_DATA_DIR", dir.join("uncreated"))
        .env_remove("CHAT_HUB_DATABASE")
        .output()
        .unwrap();
    assert!(!output.status.success());
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(!error.contains("sensitive-marker"));
    assert!(!error.contains(input.to_str().unwrap()));
    assert!(!dir.join("uncreated").exists());
    fs::remove_dir_all(dir).unwrap();
}
