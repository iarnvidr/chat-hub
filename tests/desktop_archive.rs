use chat_hub::store::Store;
use std::{
    fs,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

fn temp_dir() -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "chat-hub-desktop-archive-{}-{nonce}",
        std::process::id()
    ));
    fs::create_dir(&dir).unwrap();
    dir
}

#[test]
fn store_backup_and_json_export_publish_without_overwriting_existing_targets() {
    let dir = temp_dir();
    let db = dir.join("library.sqlite3");
    let store = Store::open(db.to_str().unwrap()).unwrap();
    let backup = dir.join("backup.sqlite3");
    store.backup_to(&backup).unwrap();
    assert!(backup.is_file());
    let backup_before = fs::read(&backup).unwrap();
    assert!(store.backup_to(&backup).is_err());
    assert_eq!(fs::read(&backup).unwrap(), backup_before);

    let export = dir.join("library.json");
    store.export_json_to(&export).unwrap();
    assert!(export.is_file());
    let json_before = fs::read(&export).unwrap();
    assert!(store.export_json_to(&export).is_err());
    assert_eq!(fs::read(&export).unwrap(), json_before);
    drop(store);
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn archive_published_errors_are_distinguished_from_failures() {
    let err = chat_hub::error::Error::ArchivePublished {
        destination: PathBuf::from("/secret/location"),
        operation: "directory sync",
        source: std::io::Error::other("secret detail"),
    };
    assert!(matches!(
        err,
        chat_hub::error::Error::ArchivePublished { .. }
    ));
    // Native command maps this durability-warning variant to a published receipt,
    // and does not expose the error/path; ordinary failures use a fixed safe string.
    let source = include_str!("../src-tauri/src/main.rs");
    let start = source.find("async fn save_library_archive(").unwrap();
    let end = source[start..].find("async fn service_health(").unwrap() + start;
    let archive = &source[start..end];
    assert!(archive
        .contains("Err(chat_hub::error::Error::ArchivePublished { .. }) => Ok(ArchiveReceipt"));
    assert!(archive.contains("durability_confirmed: false"));
    assert!(archive.contains("Err(_) => Err(\"archive_save_failed\".to_string())"));
    assert!(!archive.contains("e.to_string()"));
}

#[test]
fn native_archive_commands_authorize_before_dialog_and_are_registered() {
    let source = include_str!("../src-tauri/src/main.rs");
    for (name, kind) in [
        ("backup_library", "backup"),
        ("export_library_json", "json"),
    ] {
        let start = source.find(&format!("async fn {name}(")).unwrap();
        let end = source[start..].find("\n}\n").unwrap() + start;
        let body = &source[start..end];
        assert!(
            body.find("authorize_local_main(&window)?").unwrap()
                < body.find("save_library_archive(app, s, ").unwrap()
        );
        assert!(body.contains(&format!("save_library_archive(app, s, \"{kind}\")")));
    }
    assert!(source.contains("backup_library,\n            export_library_json"));
    let helper = source
        .split("async fn save_library_archive(")
        .nth(1)
        .unwrap();
    assert!(
        helper.find("desktop_archive::try_acquire()").unwrap() < helper.find("rx.await").unwrap()
    );
    assert!(helper.find("rx.await").unwrap() < helper.find("spawn_blocking(move ||").unwrap());
    assert!(helper.contains("let _permit = permit;"));
    assert!(include_str!("../src-tauri/src/desktop_archive.rs").contains("archive_busy"));
    assert!(helper.contains("cancelled: true"));
    assert!(helper.contains("Store::open_existing(") && helper.contains("database_path"));
    let snapshot = helper.find("let database_path = {").unwrap();
    let snapshot_end = helper[snapshot..].find("};").unwrap() + snapshot;
    let open = helper.find("Store::open_existing(").unwrap();
    assert!(
        snapshot_end < open,
        "runtime lock scope must end before opening the store"
    );
    assert!(helper[snapshot..snapshot_end].contains(".database_path\n                .clone()"));
}
