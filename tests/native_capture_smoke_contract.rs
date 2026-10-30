use chat_hub::live_capture::CaptureStatus;
use serde_json::{json, Value};
use std::io::Write;
use std::process::{Command, Stdio};

#[test]
fn actual_disabled_capture_snapshot_satisfies_smoke_contract() {
    let snapshot = serde_json::to_value(CaptureStatus::new(false).snapshot()).unwrap();
    let counters = snapshot["counters"].as_object().unwrap();
    assert_eq!(counters.len(), 19);
    assert_eq!(counters["last_error_code"], Value::Null);
    for (name, value) in counters {
        if name != "last_error_code" {
            assert_eq!(value.as_u64(), Some(0), "counter {name}");
        }
    }

    let archive = json!({"items": [], "next_cursor": null, "library_revision": 0});
    let root = env!("CARGO_MANIFEST_DIR");
    let helper = format!(
        r#"
import importlib.util, json, sys
from pathlib import Path
spec = importlib.util.spec_from_file_location('native_capture_contract', Path({helper:?}))
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)
snapshot = json.load(sys.stdin)
archive = {{'items': [], 'next_cursor': None, 'library_revision': 0}}
module.validate(snapshot, snapshot, forwarding_ok=True, archive_before=archive, archive_after=archive)
"#,
        helper = format!("{root}/build/native_capture_contract.py")
    );
    let mut child = Command::new("python3")
        .arg("-c")
        .arg(helper)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start Python contract helper");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(&serde_json::to_vec(&snapshot).unwrap())
        .unwrap();
    let result = child.wait_with_output().unwrap();
    assert!(
        result.status.success(),
        "Python helper rejected Rust serde snapshot: {}",
        String::from_utf8_lossy(&result.stderr)
    );

    let _ = archive;
}
