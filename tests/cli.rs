use std::{
    fs,
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
fn temp() -> std::path::PathBuf {
    let p = std::env::temp_dir().join(format!(
        "chat-hub-cli-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&p).unwrap();
    p
}
fn configured<'a>(cmd: &'a mut Command, d: &std::path::Path) -> &'a mut Command {
    cmd.env("CHAT_HUB_TOKEN", "cli-test-token")
        .env("CHAT_HUB_DATABASE", d.join("db.sqlite"))
        .env("CHAT_HUB_DATA_DIR", d)
        .env("CHAT_HUB_CA_DIR", d.join("ca"))
        .env("CHAT_HUB_BIND", "127.0.0.1:0")
        .env("CHAT_HUB_PROXY_ADDR", "127.0.0.1:0")
}
fn status(d: &std::path::Path) -> serde_json::Value {
    let out = configured(
        Command::new(env!("CARGO_BIN_EXE_chat-hub")).arg("status"),
        d,
    )
    .output()
    .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap()
}
struct ChildGuard(Child);
impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
#[test]
fn status_does_not_create_a_missing_database_or_runtime_files() {
    let d = temp();
    let db = d.join("db.sqlite");
    let v = status(&d);
    assert_eq!(v["service"], "stopped");
    assert_eq!(v["database_health"], serde_json::Value::Null);
    assert!(!db.exists());
    assert!(!d.join("runtime").exists());
    fs::remove_dir_all(d).unwrap();
}
#[test]
fn serve_rejects_existing_lock_without_overwriting_and_exits_cleanly() {
    let d = temp();
    let rt = d.join("runtime");
    fs::create_dir_all(&rt).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&rt, fs::Permissions::from_mode(0o700)).unwrap();
    }
    fs::write(rt.join("service.lock"), "preserve me\n").unwrap();
    let out = configured(
        Command::new(env!("CARGO_BIN_EXE_chat-hub")).arg("serve"),
        &d,
    )
    .output()
    .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("runtime lock exists"));
    assert_eq!(
        fs::read_to_string(rt.join("service.lock")).unwrap(),
        "preserve me\n"
    );
    fs::remove_dir_all(d).unwrap();
}
#[cfg(all(unix, any(target_os = "linux", target_os = "macos")))]
#[test]
fn fresh_default_startup_creates_canonical_data_dir_and_sigint_cleans_runtime() {
    let d = temp();
    let home = d.join("home");
    let xdg = d.join("xdg");
    fs::create_dir_all(&home).unwrap();
    #[cfg(target_os = "macos")]
    let expected = home.join("Library/Application Support/io.github.iarnvidr.chat-hub");
    #[cfg(target_os = "linux")]
    let expected = xdg.join("io.github.iarnvidr.chat-hub");
    assert!(!expected.exists());
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_chat-hub"));
    cmd.arg("serve")
        .env("HOME", &home)
        .env("XDG_DATA_HOME", &xdg)
        .env_remove("CHAT_HUB_DATA_DIR")
        .env_remove("CHAT_HUB_DATABASE")
        .env_remove("CHAT_HUB_CA_DIR")
        .env("CHAT_HUB_TOKEN", "cli-test-token")
        .env("CHAT_HUB_BIND", "127.0.0.1:0")
        .env("CHAT_HUB_PROXY_ADDR", "127.0.0.1:0")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = ChildGuard(cmd.spawn().unwrap());
    let record_path = expected.join("runtime/service.json");
    let deadline = Instant::now() + Duration::from_secs(10);
    let record: serde_json::Value = loop {
        if let Ok(bytes) = fs::read(&record_path) {
            if let Ok(record) = serde_json::from_slice(&bytes) {
                break record;
            }
        }
        assert!(
            Instant::now() < deadline,
            "startup did not publish runtime record"
        );
        assert!(
            child.0.try_wait().unwrap().is_none(),
            "serve exited before readiness"
        );
        thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(record["pid"].as_u64(), Some(u64::from(child.0.id())));
    let running_out = Command::new(env!("CARGO_BIN_EXE_chat-hub"))
        .arg("status")
        .env("HOME", &home)
        .env("XDG_DATA_HOME", &xdg)
        .env_remove("CHAT_HUB_DATA_DIR")
        .env_remove("CHAT_HUB_DATABASE")
        .env_remove("CHAT_HUB_CA_DIR")
        .env("CHAT_HUB_TOKEN", "cli-test-token")
        .output()
        .unwrap();
    assert!(running_out.status.success(), "status command failed");
    let running: serde_json::Value = serde_json::from_slice(&running_out.stdout).unwrap();
    assert_eq!(running["service"], "running", "{running}");
    assert_eq!(running["pid"], record["pid"], "{running}");
    assert_eq!(running["instance_id"], record["instance_id"], "{running}");
    assert_eq!(running["database_health"], true, "{running}");
    Command::new("kill")
        .args(["-INT", &child.0.id().to_string()])
        .status()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while child.0.try_wait().unwrap().is_none() {
        assert!(Instant::now() < deadline, "serve did not stop after SIGINT");
        thread::sleep(Duration::from_millis(20));
    }
    assert!(!expected.join("runtime/service.json").exists());
    fs::remove_dir_all(d).unwrap();
}

#[cfg(unix)]
#[test]
fn running_status_checks_identity_and_sigint_cleans_runtime() {
    let d = temp();
    let mut child = ChildGuard(
        configured(
            Command::new(env!("CARGO_BIN_EXE_chat-hub")).arg("serve"),
            &d,
        )
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    let record_path = d.join("runtime/service.json");
    let record: serde_json::Value = loop {
        if let Ok(bytes) = fs::read(&record_path) {
            if let Ok(v) = serde_json::from_slice(&bytes) {
                break v;
            }
        }
        assert!(
            Instant::now() < deadline,
            "serve did not publish runtime record"
        );
        assert!(
            child.0.try_wait().unwrap().is_none(),
            "serve exited before readiness"
        );
        thread::sleep(Duration::from_millis(20));
    };
    let running = status(&d);
    assert_eq!(running["service"], "running", "{running}");
    assert_eq!(running["pid"], record["pid"]);
    assert_eq!(running["instance_id"], record["instance_id"]);
    assert_eq!(running["database_health"], true);
    let original = fs::read(&record_path).unwrap();
    let mut forged = record.clone();
    forged["instance_id"] = serde_json::Value::String("forged-instance".into());
    fs::write(&record_path, serde_json::to_vec(&forged).unwrap()).unwrap();
    let mismatch = status(&d);
    assert_eq!(mismatch["service"], "identity_mismatch", "{mismatch}");
    fs::write(&record_path, original).unwrap();
    let signal = Command::new("kill")
        .args(["-INT", &child.0.id().to_string()])
        .status()
        .unwrap();
    assert!(signal.success(), "could not send SIGINT");
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if child.0.try_wait().unwrap().is_some() {
            break;
        }
        assert!(Instant::now() < deadline, "serve did not stop after SIGINT");
        thread::sleep(Duration::from_millis(20));
    }
    let stopped = status(&d);
    assert_eq!(stopped["service"], "stopped", "{stopped}");
    assert_eq!(stopped["database_health"], true);
    assert!(!record_path.exists());
    assert!(!d.join("runtime/service.token").exists());
    assert!(!d.join("runtime/service.lock").exists());
    fs::remove_dir_all(d).unwrap();
}
