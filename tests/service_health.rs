#[path = "../src-tauri/src/service_health.rs"]
mod service_health;

#[test]
fn requires_exact_http_200_and_ok_json() {
    let good = serde_json::json!({"status":"ok"});
    assert!(service_health::healthy_response(200, &good));
    for status in [201, 401, 503] {
        assert!(!service_health::healthy_response(status, &good));
    }
    assert!(!service_health::healthy_response(
        200,
        &serde_json::json!({"status":"bad"})
    ));
    assert!(!service_health::healthy_response(
        200,
        &serde_json::json!({"other":true})
    ));
}
