mod common;

use chat_hub::store::{GraphNode, Observation, Store};
use serde_json::{json, Value};
use std::{fs, process::Command};
fn invoke(command: &str, extra: &[&str], db: &std::path::Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_chat-hub"))
        .arg(command)
        .args(extra)
        .env("CHAT_HUB_TOKEN", "synthetic-token")
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
            parts: vec![Value::String("x".repeat(96 * 1024))],
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
    fs::remove_dir_all(d).unwrap();
}
