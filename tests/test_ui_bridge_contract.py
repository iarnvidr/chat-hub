import json
import pathlib
import plistlib
import re
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[1]


def rust_function(source, name):
    """Return a Rust function signature and body while ignoring braces in strings."""
    matches = list(re.finditer(rf"\bfn\s+{re.escape(name)}\s*\(", source))
    if not matches:
        return None
    match = matches[-1]
    open_paren = source.find("(", match.start())
    depth = 0
    quote = False
    escaped = False
    close_paren = None
    for index in range(open_paren, len(source)):
        char = source[index]
        if quote:
            if escaped:
                escaped = False
            elif char == "\\":
                escaped = True
            elif char == '"':
                quote = False
            continue
        if char == '"':
            quote = True
        elif char == "(":
            depth += 1
        elif char == ")":
            depth -= 1
            if depth == 0:
                close_paren = index
                break
    if close_paren is None:
        return None
    body_start = source.find("{", close_paren)
    if body_start < 0:
        return None
    depth = 0
    quote = False
    escaped = False
    for index in range(body_start, len(source)):
        char = source[index]
        if quote:
            if escaped:
                escaped = False
            elif char == "\\":
                escaped = True
            elif char == '"':
                quote = False
            continue
        if char == '"':
            quote = True
        elif char == "{":
            depth += 1
        elif char == "}":
            depth -= 1
            if depth == 0:
                return source[match.start():body_start], source[body_start + 1:index]
    return None


class UiBridgeContract(unittest.TestCase):
    def test_local_ui_uses_tauri_internal_invoke_and_calls_real_service(self):
        config = json.loads((ROOT / "src-tauri/tauri.conf.json").read_text())
        self.assertFalse(config["app"]["withGlobalTauri"])
        self.assertEqual(config["build"]["frontendDist"], "../desktop-ui")
        windows = config["app"]["windows"]
        self.assertEqual([window.get("label", "main") for window in windows], ["main"])
        html = (ROOT / "desktop-ui/index.html").read_text()
        self.assertIn('<script type="module" src="./app.js"></script>', html)
        app = (ROOT / "desktop-ui/app.js").read_text()
        self.assertIn("window.__TAURI_INTERNALS__?.invoke", app)
        self.assertIn("const status=()=>updateStatus({invoke:native?invoke:null", app)
        status = (ROOT / "desktop-ui/status.js").read_text()
        self.assertIn("await invoke('service_info')", status)
        self.assertIn("invoke('service_health')", status)
        self.assertIn("window.ChatHub={invoke}", app)
        capture_ui = (ROOT / "desktop-ui/capture-status.js").read_text()
        self.assertIn("get_capture_preferences", capture_ui)
        self.assertIn("set_capture_preferences", capture_ui)
        self.assertIn("restart_capture_settings", capture_ui)
        self.assertIn('id="settings-page"', html)
        self.assertIn('id="capture-title"', html)
        self.assertIn('id="capture-enabled-toggle"', html)
        self.assertIn("Save live chats on this Mac", html)
        tauri = (ROOT / "src-tauri/src/main.rs").read_text()
        self.assertIn("service_info,", tauri)
        signature, body = rust_function(tauri, "service_info")
        self.assertIn("window: tauri::Webview", signature)
        self.assertIn("authorize_local_main(&window)?;", body)

    def test_provider_is_created_as_an_external_child_webview(self):
        main = (ROOT / "src-tauri/src/main.rs").read_text()
        provider = (ROOT / "src-tauri/src/provider_view.rs").read_text()
        self.assertIn("ProviderViewState::attach(", main)
        self.assertIn('WebviewBuilder::new("provider", tauri::WebviewUrl::External(initial_url))', provider)
        self.assertIn(".add_child(", provider)
        self.assertIn('tauri::Url::parse(&format!("https://chatgpt.com{path}"))', provider)
        self.assertNotIn("window.__TAURI_INTERNALS__", provider)
        config = json.loads((ROOT / "src-tauri/tauri.conf.json").read_text())
        self.assertEqual(config["app"]["windows"][0]["title"], "Chat Hub")

    def test_macos_transport_exception_is_scoped_to_chatgpt(self):
        plist = plistlib.loads((ROOT / "src-tauri/Info.plist").read_bytes())
        transport = plist["NSAppTransportSecurity"]
        self.assertTrue(transport["NSAllowsLocalNetworking"])
        self.assertNotIn("NSAllowsArbitraryLoads", transport)
        self.assertNotIn("NSAllowsArbitraryLoadsInWebContent", transport)
        exceptions = transport["NSExceptionDomains"]
        self.assertEqual(set(exceptions), {"chatgpt.com"})
        self.assertFalse(exceptions["chatgpt.com"]["NSIncludesSubdomains"])
        self.assertTrue(exceptions["chatgpt.com"]["NSExceptionAllowsInsecureHTTPLoads"])

    def test_sensitive_commands_are_bound_to_authorized_main_webview(self):
        source = (ROOT / "src-tauri/src/main.rs").read_text()
        sensitive = (
            "update_status", "service_info", "check_for_updates", "install_update",
            "e2e_result", "set_provider_view", "open_provider_conversation",
            "get_chatgpt_connection", "prepare_chatgpt_session",
            "set_conversation_project",
            "inspect_project_client",
            "learned_project_operations",
            "rename_project",
            "catalog_overview", "catalog_graph", "rebuild_catalog_index",
            "get_capture_preferences", "set_capture_preferences", "restart_capture_settings",
            "sync_chatgpt_history", "chatgpt_history_status",
            "sync_chatgpt_pins",
        )
        for command in sensitive:
            with self.subTest(command=command):
                function = rust_function(source, command)
                self.assertIsNotNone(function, f"missing command {command}")
                signature, body = function
                self.assertIn("window: tauri::Webview", signature)
                self.assertIn("authorize_local_main(&window)?;", body)

    def test_chatgpt_pin_snapshot_is_main_only_and_read_only(self):
        main = (ROOT / "src-tauri/src/main.rs").read_text()
        function = rust_function(main, "sync_chatgpt_pins")
        self.assertIsNotNone(function)
        _, body = function
        first_statement = next(line.strip() for line in body.splitlines() if line.strip())
        self.assertEqual(first_statement, "authorize_local_main(&window)?;")
        self.assertNotIn("history.snapshot()", body)
        self.assertIn("history.note_provider_rate_limit", body)
        handler_blocks = re.findall(r"\.invoke_handler\(tauri::generate_handler!\[(.*?)\]\)", main, re.S)
        self.assertEqual(len(handler_blocks), 2)
        for handler in handler_blocks:
            self.assertIn("sync_chatgpt_pins", handler)
        worker = (ROOT / "src-tauri/src/pin_snapshot.js").read_text()
        provider = (ROOT / "src-tauri/src/provider_view.rs").read_text()
        self.assertIn("/backend-api/pins?item_type=conversation", worker)
        self.assertNotRegex(worker, r"method:\s*['\"](?:POST|DELETE)['\"]")
        self.assertIn("payload[key] !== rows.length", worker)
        self.assertIn("pin_snapshot_cooldown_remaining(expected_scope)", provider)

    def test_library_item_commands_authorize_before_any_file_or_service_io(self):
        source = (ROOT / "src-tauri/src/library_items.rs").read_text()
        commands = re.findall(r"pub async fn\s+(\w+)\s*\(", source)
        self.assertEqual(
            set(commands),
            {
                "catalog_entities", "catalog_entity", "list_local_tags",
                "catalog_entity_graph", "catalog_tag_suggestions",
                "get_target_tags", "set_target_tags", "tagged_targets",
                "saved_artifacts", "attach_artifact_file", "export_artifact_file",
                "open_artifact_file", "open_catalog_link",
            },
        )
        for command in commands:
            with self.subTest(command=command):
                function = rust_function(source, command)
                self.assertIsNotNone(function)
                signature, body = function
                self.assertIn("window: tauri::Webview", signature)
                first_statement = next(line.strip() for line in body.splitlines() if line.strip())
                self.assertEqual(first_statement, "authorize_local_main(&window)?;")

    def test_chatgpt_project_assignment_is_server_verified_before_local_commit(self):
        main = (ROOT / "src-tauri/src/main.rs").read_text()
        function = rust_function(main, "set_conversation_project")
        self.assertIsNotNone(function)
        _, body = function
        self.assertIn('unwrap_or("upstream")', body)
        self.assertIn('scope == "local"', body)
        self.assertIn(".set_conversation_project(", body)
        self.assertLess(body.index('if outcome.state != "applied"'), body.rindex("commit_project_assignment("))
        self.assertIn('"verified":true', body)
        worker = (ROOT / "src-tauri/src/project_assignment.js").read_text()
        self.assertIn('method: "PATCH"', worker)
        self.assertIn("await readConversation()", worker)
        self.assertIn("actual !== targetProjectId", worker)
        self.assertIn("Authorization: `Bearer ${token}`", worker)

    def test_project_client_probe_is_debug_only_and_read_only(self):
        main = (ROOT / "src-tauri/src/main.rs").read_text()
        function = rust_function(main, "inspect_project_client")
        self.assertIsNotNone(function)
        _, body = function
        self.assertIn("cfg!(debug_assertions)", body)
        self.assertIn("project_client_probe::inspect(view)", body)
        probe = (ROOT / "src-tauri/src/project_client_probe.js").read_text()
        self.assertIn('credentials: "omit"', probe)
        self.assertNotIn("Authorization", probe)
        self.assertNotIn("document.cookie", probe)

    def test_catalog_graph_optional_focus_id_is_forwarded_end_to_end(self):
        main = (ROOT / "src-tauri/src/main.rs").read_text()
        function = rust_function(main, "catalog_graph")
        self.assertIsNotNone(function)
        signature, body = function
        self.assertIn("focus_id: Option<String>", signature)
        self.assertIn("focus_id.as_deref()", body)
        api = (ROOT / "src-tauri/src/desktop_api.rs").read_text()
        self.assertIn('query.push(("focus_id", focus_id.to_owned()))', api)
        server = (ROOT / "src/server/api_v1.rs").read_text()
        self.assertRegex(server, r"store\.catalog_neighborhood_tagged\(\s*focus_id,\s*&source,\s*project_key\.as_deref\(\),\s*tag\.as_deref\(\),\s*limit,?\s*\)")

    def test_guard_requires_live_native_main_ownership_and_local_origin(self):
        source = (ROOT / "src-tauri/src/main.rs").read_text()
        function = rust_function(source, "authorize_local_main")
        self.assertIsNotNone(function)
        signature, guard = function
        self.assertIn("window: &tauri::Webview", signature)
        self.assertIn('window.label() != "main"', guard)
        self.assertIn("window.app_handle().state::<MainWindowOwnership>()", guard)
        self.assertIn("ownership.is_authorized(window.label())", guard)
        self.assertIn("window_policy::allows_origin(", source)
        policy = (ROOT / "src-tauri/src/window_policy.rs").read_text()
        self.assertIn('scheme == "tauri" && host == Some("localhost")', policy)
        self.assertIn('scheme == "http" && host == Some("tauri.localhost")', policy)
        self.assertIn('username.is_empty()', policy)
        self.assertIn('password.is_none()', policy)
        self.assertIn('port.is_none()', policy)
        self.assertIn("main_ownership.register_after_build()", source)

    def test_project_rename_defaults_to_upstream_and_fails_closed_until_verified(self):
        source = (ROOT / "src-tauri/src/main.rs").read_text()
        function = rust_function(source, "rename_project")
        self.assertIsNotNone(function)
        signature, body = function
        self.assertIn("scope: Option<String>", signature)
        self.assertIn('unwrap_or("upstream")', body)
        self.assertIn('"local" =>', body)
        self.assertIn('"upstream" =>', body)
        self.assertIn('outcome.state != "applied" || !outcome.verified', body)
        self.assertIn('desktop_api::commit_verified_project_title(', body)
        self.assertLess(body.index('outcome.state != "applied" || !outcome.verified'), body.index('desktop_api::commit_verified_project_title('))
        self.assertIn('observed_scope != project_target.account_scope', body)
        self.assertIn("WindowEvent::Destroyed", source)
        self.assertIn("ownership.revoke()", source)

    def test_provider_smoke_denial_is_not_a_production_command(self):
        source = (ROOT / "src-tauri/src/main.rs").read_text()
        self.assertIn("app.get_webview(\"provider\")", source)
        self.assertIn("window_policy::is_provider_origin(u.scheme()", source)
        self.assertIn('"provider webview readiness timed out"', source)
        self.assertIn('"readiness":readiness', source)
        handlers = source.split(".invoke_handler", 1)[-1]
        self.assertRegex(handlers, r"e2e_result\s*,?")
        production_handlers = handlers.split('#[cfg(not(feature = "native-smoke"))]', 1)[-1]
        for command in ("get_capture_preferences", "set_capture_preferences", "restart_capture_settings"):
            self.assertIn(command, production_handlers)
        self.assertNotIn("e2e_result", production_handlers)
        self.assertNotIn("e2e_provider_result", production_handlers)


if __name__ == "__main__":
    unittest.main()
