mod common;

use chat_hub::store::Store;
use std::{fs, process::Command};

fn cli(args: &[&str], data: &std::path::Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_chat-hub"))
        .args(args)
        .env_remove("CHAT_HUB_TOKEN")
        .env_remove("CHAT_HUB_DATABASE")
        .env("CHAT_HUB_DATA_DIR", data)
        .output()
        .unwrap()
}

#[test]
fn help_version_and_usage_work_without_configuration_or_side_effects() {
    let dir = common::temp_dir("chat-hub-cli-help");
    let data = dir.join("not-created");
    for args in [&["--help"][..], &["--version"][..], &[][..]] {
        let result = cli(args, &data);
        assert!(result.status.success(), "{result:?}");
        assert!(
            String::from_utf8_lossy(&result.stdout).contains("Chat Hub")
                || String::from_utf8_lossy(&result.stdout).contains("chat-hub")
        );
    }
    for args in [
        &["search"][..],
        &["read", "id", "extra"][..],
        &["status", "extra"][..],
    ] {
        let result = cli(args, &data);
        assert!(!result.status.success());
        assert!(String::from_utf8_lossy(&result.stderr).contains("usage:"));
    }
    let missing = cli(&["search", "test"], &data);
    assert!(!missing.status.success());
    assert!(!data.exists());
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn local_import_search_and_read_need_no_service_token() {
    let dir = common::temp_dir("chat-hub-cli-local");
    let data = dir.join("library");
    let input = dir.join("export.json");
    fs::write(&input, include_bytes!("fixtures/chatgpt-export/array.json")).unwrap();
    let imported = cli(&["import", input.to_str().unwrap()], &data);
    assert!(imported.status.success(), "{imported:?}");
    let store = Store::open_existing(data.join("chat-hub.sqlite3").to_str().unwrap()).unwrap();
    let snapshot = store.library_snapshot().unwrap();
    let id = snapshot.conversations[0]["id"].as_str().unwrap().to_owned();
    drop(store);
    let read = cli(&["read", &id], &data);
    assert!(read.status.success(), "{read:?}");
    let body: serde_json::Value = serde_json::from_slice(&read.stdout).unwrap();
    assert!(body.is_object());
    let search = cli(&["search", "synthetic"], &data);
    assert!(search.status.success(), "{search:?}");
    assert!(String::from_utf8_lossy(&search.stdout)
        .lines()
        .any(|line| line == id));
    let status = cli(&["status"], &data);
    assert!(status.status.success(), "{status:?}");
    let status: serde_json::Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(status["database_health"], true);
    fs::remove_dir_all(dir).unwrap();
}
