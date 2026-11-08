"""Guard compilation ownership independently of the Dagger implementation.

Production edges define the application graph. Test-only adapters are deliberate:
capture tests may use SQLite without adding SQLite to the capture library.
"""

from pathlib import Path
import tomllib
import unittest


ROOT = Path(__file__).resolve().parents[1]
EXPECTED_EDGES = {
    "chat-hub-model": set(),
    "chat-hub-formats": {"chat-hub-model"},
    "chat-hub-store": {"chat-hub-model", "chat-hub-formats"},
    "chat-hub-ingest": {"chat-hub-store", "chat-hub-formats"},
    "chat-hub-capture": {"chat-hub-model", "chat-hub-formats"},
    "chat-hub-api": {"chat-hub-model", "chat-hub-store", "chat-hub-ingest"},
    "chat-hub-runtime": {
        "chat-hub-model", "chat-hub-store", "chat-hub-capture", "chat-hub-api"
    },
}


def manifest(path):
    with path.open("rb") as stream:
        return tomllib.load(stream)


def production_dependencies(document):
    """Include build scripts and target-specific dependencies, not just Linux."""
    tables = [document, *document.get("target", {}).values()]
    result = {}
    for table in tables:
        for kind in ("dependencies", "build-dependencies"):
            for alias, spec in table.get(kind, {}).items():
                name = spec.get("package", alias) if isinstance(spec, dict) else alias
                result[name] = spec
    return result


class WorkspaceArchitecture(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.root_manifest = manifest(ROOT / "Cargo.toml")
        cls.packages = {}
        for member in cls.root_manifest["workspace"]["members"]:
            path = ROOT / member / "Cargo.toml"
            document = manifest(path)
            cls.packages[document["package"]["name"]] = (path, document)

    def test_workspace_contains_implemented_owner_crates(self):
        self.assertEqual(
            set(self.packages), set(EXPECTED_EDGES) | {"chat-hub", "chat-hub-desktop"}
        )
        for name in EXPECTED_EDGES:
            with self.subTest(crate=name):
                path, document = self.packages[name]
                self.assertTrue((path.parent / "src/lib.rs").is_file())
                self.assertFalse(document["package"].get("publish", True))

    def test_production_edges_are_explicit_and_point_towards_lower_layers(self):
        for name, allowed in EXPECTED_EDGES.items():
            with self.subTest(crate=name):
                path, document = self.packages[name]
                dependencies = production_dependencies(document)
                actual = set(dependencies) & set(self.packages)
                self.assertEqual(actual, allowed)
                for dependency in actual:
                    spec = dependencies[dependency]
                    self.assertIn("path", spec)
                    self.assertEqual(
                        (path.parent / spec["path"] / "Cargo.toml").resolve(),
                        self.packages[dependency][0].resolve(),
                    )

    def test_production_graph_has_no_cycles(self):
        graph = {
            name: set(production_dependencies(document)) & set(self.packages)
            for name, (_, document) in self.packages.items()
        }

        def visit(name, active):
            self.assertNotIn(name, active, f"cyclic crate graph: {active} -> {name}")
            for dependency in graph[name]:
                visit(dependency, (*active, name))

        for name in graph:
            visit(name, ())

    def test_model_stays_dependency_light(self):
        self.assertEqual(
            set(production_dependencies(self.packages["chat-hub-model"][1])),
            {"serde", "serde_json"},
        )

    def test_capture_and_formats_do_not_link_a_database_or_native_ui(self):
        forbidden = {"rusqlite", "sqlx", "tauri", "chat-hub", "chat-hub-desktop"}
        for name in ("chat-hub-capture", "chat-hub-formats"):
            dependencies = production_dependencies(self.packages[name][1])
            self.assertFalse(set(dependencies) & forbidden, name)
            self.assertNotIn("chat-hub-store", dependencies, name)

    def test_storage_has_no_transport_or_application_dependency(self):
        dependencies = production_dependencies(self.packages["chat-hub-store"][1])
        self.assertFalse(
            set(dependencies) & {"axum", "hyper", "reqwest", "tauri", "chat-hub-runtime"}
        )

    def test_root_library_is_a_facade_not_a_second_core(self):
        source = (ROOT / "src/lib.rs").read_text()
        self.assertIn("pub use chat_hub_runtime", source)
        self.assertIn("pub use chat_hub_store", source)
        self.assertNotIn("pub mod store", source)
        self.assertNotIn("pub mod proxy", source)
        for old_module in ("store.rs", "proxy.rs", "server.rs", "service.rs"):
            self.assertFalse((ROOT / "src" / old_module).exists(), old_module)

    def test_api_reports_application_version_after_extraction(self):
        version = self.packages["chat-hub"][1]["package"]["version"]
        for name in ("chat-hub-api", "chat-hub-desktop"):
            self.assertEqual(self.packages[name][1]["package"]["version"], version)


if __name__ == "__main__":
    unittest.main()
