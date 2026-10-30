#[path = "../src-tauri/src/service_health.rs"]
mod service_health;

#[test]
fn requires_exact_http_200_and_healthy_v1_payload() {
    let good = serde_json::json!({"status":"healthy","service":{"status":"ready"},"database":{"status":"healthy"}});
    assert!(service_health::healthy_response(200, &good));
    for status in [201, 401, 503] {
        assert!(!service_health::healthy_response(status, &good));
    }
    assert!(!service_health::healthy_response(
        200,
        &serde_json::json!({"status":"ok","service":{"status":"ready"},"database":{"status":"healthy"}})
    ));
    assert!(!service_health::healthy_response(
        200,
        &serde_json::json!({"other":true})
    ));
}
