use chat_hub::config::{APP_IDENTIFIER, APP_NAME};
use serde_json::Value;

#[test]
fn cli_and_desktop_share_the_canonical_application_identity() {
    let desktop: Value = serde_json::from_str(include_str!("../src-tauri/tauri.conf.json"))
        .expect("Tauri config is valid JSON");
    let package = include_str!("../Cargo.toml");
    let desktop_package = include_str!("../src-tauri/Cargo.toml");

    assert_eq!(APP_NAME, "chat-hub");
    assert_eq!(desktop["productName"], "Chat Hub");
    assert_eq!(desktop["identifier"], APP_IDENTIFIER);
    assert!(package.contains("name = \"chat-hub\""));
    assert!(desktop_package.contains("name = \"chat-hub-desktop\""));
    assert!(APP_IDENTIFIER.starts_with("io.github."));
}
