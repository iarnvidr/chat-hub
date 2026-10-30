pub fn healthy_response(status: u16, value: &serde_json::Value) -> bool {
    status == 200 && value.get("status").and_then(|v| v.as_str()) == Some("ok")
}
