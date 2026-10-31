mod common;

use chat_hub::store::{GraphNode, Observation, Store};
use serde_json::{json, Value};
use std::{fs, process::Command};
fn invoke(command: &str, extra: &[&str], db: &std::path::Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_chat-hub"))
        .arg(command)
        .args(extra)
        .env_remove("CHAT_HUB_TOKEN")
        .env("CHAT_HUB_DATABASE", db)
        .output()
        .unwrap()
}
fn populate(path: &std::path::Path) {
    let mut s = Store::open(path.to_str().unwrap()).unwrap();
    s.ingest_observation(&Observation {
        provider: "fixture".into(),
        account_scope: "scope".into(),
        source_id: "one".into(),
        title: "fixture".into(),
        url: None,
        current_node: Some("n".into()),
        graph: vec![GraphNode {
            source_node_id: "n".into(),
            source_message_id: Some("m".into()),
            parent_id: None,
            role: Some("user".into()),
            author: None,
            create_time: None,
            parts: vec![Value::String(format!(
                "large body marker {}",
                "x".repeat(96 * 1024)
            ))],
        }],
        raw_payload: json!({"private":"not-in-receipt"}),
        source_time: None,
        observed_ms: 1,
        parser: "fixture".into(),
        completeness: "complete".into(),
    })
    .unwrap();
}
#[test]
fn cli_backup_and_export_json_use_store_and_emit_only_receipts() {
    let d = common::temp_dir("chat-hub-cli-archive");
    let db = d.join("source.sqlite");
    populate(&db);
    for (cmd, dest) in [
        ("backup", d.join("backup.sqlite")),
        ("export-json", d.join("export.json")),
    ] {
        let out = invoke(cmd, &[dest.to_str().unwrap()], &db);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let receipt: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(receipt["operation"], cmd);
        assert_eq!(receipt["conversation_count"], 1);
        assert!(!String::from_utf8_lossy(&out.stdout).contains("not-in-receipt"));
        if cmd == "backup" {
            let s = Store::open(dest.to_str().unwrap()).unwrap();
            assert_eq!(s.library_snapshot().unwrap().conversations.len(), 1);
        } else {
            let v: Value = serde_json::from_slice(&fs::read(&dest).unwrap()).unwrap();
            assert_eq!(v["conversations"].as_array().unwrap().len(), 1);
            assert!(fs::metadata(&dest).unwrap().len() > 96 * 1024);
        }
        let no_clobber = invoke(cmd, &[dest.to_str().unwrap()], &db);
        assert!(!no_clobber.status.success());
    }
    let missing = invoke("backup", &[], &db);
    assert!(!missing.status.success());
    let extra = invoke("export-json", &["one", "two"], &db);
    assert!(!extra.status.success());
    let same = invoke("backup", &[db.to_str().unwrap()], &db);
    assert!(!same.status.success());
    // Neither archive source opening nor destination/staging setup may create paths.
    for cmd in ["backup", "export-json"] {
        let absent_parent = d.join("absent-parent").join("library.sqlite");
        let out = invoke(
            cmd,
            &[d.join("destination").to_str().unwrap()],
            &absent_parent,
        );
        assert!(
            !out.status.success(),
            "{cmd} unexpectedly created absent source"
        );
        assert!(!absent_parent.parent().unwrap().exists());
        let absent = d.join("missing.sqlite");
        let nested_dest = d.join("not-created").join("archive");
        let out = invoke(cmd, &[nested_dest.to_str().unwrap()], &absent);
        assert!(
            !out.status.success(),
            "{cmd} unexpectedly opened missing source"
        );
        assert!(!absent.exists());
        assert!(!nested_dest.parent().unwrap().exists());
        let memory_dest = d.join("memory-destination");
        let out = Command::new(env!("CARGO_BIN_EXE_chat-hub"))
            .arg(cmd)
            .arg(&memory_dest)
            .env("CHAT_HUB_TOKEN", "synthetic-token")
            .env("CHAT_HUB_DATABASE", ":memory:")
            .output()
            .unwrap();
        assert!(
            !out.status.success(),
            "{cmd} accepted ephemeral memory source"
        );
        assert!(!memory_dest.exists());
    }
    let empty = d.join("empty.sqlite");
    drop(Store::open(empty.to_str().unwrap()).unwrap());
    for (cmd, suffix) in [
        ("backup", "empty-backup.sqlite"),
        ("export-json", "empty-export.json"),
    ] {
        let dest = d.join(suffix);
        let out = invoke(cmd, &[dest.to_str().unwrap()], &empty);
        assert!(
            out.status.success(),
            "empty {cmd}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    fs::remove_dir_all(d).unwrap();
}

#[test]
fn cli_restore_populates_an_empty_library_and_rejects_replacement() {
    let d = common::temp_dir("chat-hub-cli-restore");
    let source = d.join("source.sqlite");
    let backup = d.join("backup.sqlite");
    let destination = d.join("restored.sqlite");
    populate(&source);
    let backup_output = invoke("backup", &[backup.to_str().unwrap()], &source);
    assert!(backup_output.status.success());

    let restored = invoke("restore", &[backup.to_str().unwrap()], &destination);
    assert!(
        restored.status.success(),
        "{}",
        String::from_utf8_lossy(&restored.stderr)
    );
    let receipt: Value = serde_json::from_slice(&restored.stdout).unwrap();
    assert_eq!(receipt["operation"], "restore");
    assert_eq!(receipt["conversation_count"], 1);
    let library = Store::open(destination.to_str().unwrap()).unwrap();
    assert_eq!(library.search("large body marker", 10).unwrap().len(), 1);
    drop(library);

    let second = invoke("restore", &[backup.to_str().unwrap()], &destination);
    assert!(
        !second.status.success(),
        "restore must not overwrite existing records"
    );
    let wrong_usage = invoke(
        "restore",
        &[backup.to_str().unwrap(), "extra"],
        &d.join("unused.sqlite"),
    );
    assert!(!wrong_usage.status.success());
    std::fs::remove_dir_all(d).unwrap();
}
