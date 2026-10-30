#[path = "../src-tauri/src/window_policy.rs"]
mod window_policy;

use url::Url;

#[test]
fn parsed_origins_are_strict() {
    use window_policy::{allows_origin, is_provider_origin};

    for (raw, expected) in [
        ("tauri://localhost", true),
        ("http://tauri.localhost", true),
        ("https://tauri.localhost", false),
        ("http://tauri.localhost:8080", false),
        ("http://tauri.localhost:443", false),
        ("http://user@tauri.localhost", false),
        ("http://user:pw@tauri.localhost", false),
        ("http://tauri.localhost.evil", false),
    ] {
        let url = Url::parse(raw).unwrap();
        assert_eq!(
            allows_origin(
                url.scheme(),
                url.host_str(),
                url.port(),
                url.username(),
                url.password()
            ),
            expected,
            "{raw}"
        );
    }

    for (raw, expected) in [
        ("https://chatgpt.com", true),
        ("https://chatgpt.com/path?q=x", true),
        ("https://chatgpt.com.evil/", false),
        ("https://chatgpt.com.evil@evil/", false),
        ("https://chatgpt.com:444/", false),
        ("https://chatgpt.com:443/", true),
        ("https://chatgpt.com./", false),
        ("http://chatgpt.com/", false),
    ] {
        let url = Url::parse(raw).unwrap();
        assert_eq!(
            is_provider_origin(
                url.scheme(),
                url.host_str(),
                url.port(),
                url.username(),
                url.password()
            ),
            expected,
            "{raw}"
        );
    }
}
