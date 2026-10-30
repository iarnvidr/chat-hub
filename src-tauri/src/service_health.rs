pub fn healthy_response(status: u16, value: &serde_json::Value) -> bool {
    status == 200
        && value["status"].as_str() == Some("healthy")
        && value["service"]["status"].as_str() == Some("ready")
        && value["database"]["status"].as_str() == Some("healthy")
}
