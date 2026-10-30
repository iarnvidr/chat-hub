mod common;
#[path = "../src-tauri/src/e2e_protocol.rs"]
mod e2e_protocol;

use e2e_protocol::{publish_dispatch, publish_terminal, DispatchReply};
use serde_json::{json, Value};
use std::{
    fs,
    io::ErrorKind,
    path::PathBuf,
    sync::{Arc, Barrier},
    thread,
};

fn scratch() -> PathBuf {
    common::temp_dir("huginn-e2e-protocol")
}

fn terminal(dir: &std::path::Path, id: &str, value: &Value) {
    publish_dispatch(dir, id, DispatchReply::Terminal(value.clone())).unwrap();
}

#[test]
fn terminal_before_ack_and_ack_before_terminal_keep_independent_files() {
    let d = scratch();
    terminal(&d, "terminal-first", &json!({"value":1}));
    publish_dispatch(&d, "terminal-first", DispatchReply::Deferred).unwrap();
    assert_eq!(
        fs::read(d.join("terminal-first.res.json")).unwrap(),
        br#"{"value":1}"#
    );
    assert_eq!(
        fs::read(d.join("terminal-first.ack.json")).unwrap(),
        br#"{"ok":true}"#
    );

    publish_dispatch(&d, "ack-first", DispatchReply::Deferred).unwrap();
    assert!(!d.join("ack-first.res.json").exists());
    terminal(&d, "ack-first", &json!({"value":2}));
    assert_eq!(
        fs::read(d.join("ack-first.res.json")).unwrap(),
        br#"{"value":2}"#
    );
    assert_eq!(fs::read_dir(&d).unwrap().count(), 4);
    fs::remove_dir_all(d).unwrap();
}

#[test]
fn concurrent_ack_and_terminal_publish_independent_complete_results() {
    let d = scratch();
    let barrier = Arc::new(Barrier::new(3));
    let ack_dir = d.clone();
    let ack_barrier = barrier.clone();
    let ack = thread::spawn(move || {
        ack_barrier.wait();
        publish_dispatch(&ack_dir, "race", DispatchReply::Deferred).unwrap();
    });
    let result_dir = d.clone();
    let result_barrier = barrier.clone();
    let result = thread::spawn(move || {
        result_barrier.wait();
        terminal(&result_dir, "race", &json!({"ok":true}));
    });
    barrier.wait();
    ack.join().unwrap();
    result.join().unwrap();
    assert_eq!(
        fs::read(d.join("race.ack.json")).unwrap(),
        br#"{"ok":true}"#
    );
    assert_eq!(
        fs::read(d.join("race.res.json")).unwrap(),
        br#"{"ok":true}"#
    );
    assert_eq!(fs::read_dir(&d).unwrap().count(), 2);
    fs::remove_dir_all(d).unwrap();
}

#[test]
fn concurrent_distinct_ids_do_not_corrupt_and_duplicate_terminal_is_first_writer_wins() {
    let d = scratch();
    let barrier = Arc::new(Barrier::new(9));
    let workers: Vec<_> = (0..8)
        .map(|i| {
            let d = d.clone();
            let b = barrier.clone();
            thread::spawn(move || {
                b.wait();
                terminal(&d, &format!("id-{i}"), &json!({"id":i}));
            })
        })
        .collect();
    barrier.wait();
    for worker in workers {
        worker.join().unwrap();
    }
    for i in 0..8 {
        let value: Value =
            serde_json::from_slice(&fs::read(d.join(format!("id-{i}.res.json"))).unwrap()).unwrap();
        assert_eq!(value, json!({"id":i}));
    }
    let duplicate = publish_terminal(&d, "id-0", &json!({"id":"replacement"}));
    assert_eq!(duplicate.unwrap_err().kind(), ErrorKind::AlreadyExists);
    let value: Value = serde_json::from_slice(&fs::read(d.join("id-0.res.json")).unwrap()).unwrap();
    assert_eq!(value, json!({"id":0}));
    assert_eq!(fs::read_dir(&d).unwrap().count(), 8);
    fs::remove_dir_all(d).unwrap();
}

#[test]
fn dispatch_error_is_terminal_json_without_ack_and_bad_ids_never_escape_directory() {
    let d = scratch();
    let error = json!({"ok":false,"error":"dispatch rejected"});
    terminal(&d, "error", &error);
    let value: Value =
        serde_json::from_slice(&fs::read(d.join("error.res.json")).unwrap()).unwrap();
    assert_eq!(value, error);
    assert!(!d.join("error.ack.json").exists());
    assert_eq!(
        publish_dispatch(&d, "../escape", DispatchReply::Deferred)
            .unwrap_err()
            .kind(),
        ErrorKind::InvalidInput
    );
    assert!(!d.parent().unwrap().join("escape.ack.json").exists());
    fs::remove_dir_all(d).unwrap();
}

#[test]
fn failed_destination_does_not_leave_result_or_temp_files() {
    let d = scratch().join("missing");
    assert!(publish_dispatch(&d, "missing", DispatchReply::Deferred).is_err());
    assert!(!d.join("missing.ack.json").exists());
    assert!(publish_terminal(&d, "missing", &json!({"ok":false})).is_err());
    assert!(!d.join("missing.res.json").exists());
}
