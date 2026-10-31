"""Production build-chain contracts: real bundle, ad-hoc mode, evidence, gated publish."""
import unittest
import json
import os
import subprocess
import tarfile
import tempfile
import textwrap
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


class ProductionContract(unittest.TestCase):
    def test_main_transport_stages_the_actual_producer_artifact_paths(self):
        action = (ROOT / "ci/actions/macos-native-build/action.yml").read_text()
        step = action.split("- name: Stage immutable GitHub Packages transport archive\n", 1)[1].split("    - id: publish-transport", 1)[0]
        script = textwrap.dedent(step.split("      run: |\n", 1)[1])
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            workspace, runner = root / "workspace", root / "runner"
            artifacts = workspace / "artifacts"
            artifacts.mkdir(parents=True)
            app = runner / "chat-hub/Chat Hub.app/Contents"
            app.mkdir(parents=True)
            (app / "Info.plist").write_text("fixture app")
            safe = runner / "chat-hub-safe-evidence"
            safe.mkdir()
            (safe / "receipt.json").write_text("{}")
            (runner / "chat-hub/verify-macos.txt").write_text("verified")
            name = "Chat_Hub_0.2.1_macos-arm64"
            expected = {name + ".zip": b"accepted archive", name + ".zip.sha256": b"checksum", name + ".dmg": b"accepted DMG", "native-receipt.json": b"{}"}
            for filename, data in expected.items():
                (artifacts / filename).write_bytes(data)
            (runner / ("chat-hub/" + name + ".dmg")).write_bytes(expected[name + ".dmg"])
            env = {**os.environ, "RUNNER_TEMP": str(runner), "GITHUB_REPOSITORY": "iarnvidr/chat-hub", "GITHUB_RUN_ID": "123", "GITHUB_RUN_ATTEMPT": "2", "GITHUB_SHA": "a" * 40}
            subprocess.run(["bash", "-e", "-c", script], cwd=workspace, env=env, check=True, capture_output=True)
            transport = runner / "chat-hub-transport"
            for filename, data in expected.items():
                self.assertEqual((transport / "chat-hub-native-release-main-123-2" / filename).read_bytes(), data)
            self.assertEqual((transport / "chat-hub-macos-arm64" / (name + ".zip")).read_bytes(), expected[name + ".zip"])
            self.assertEqual(json.loads((transport / "transport.json").read_text()), {"repository": "iarnvidr/chat-hub", "run_id": "123", "run_attempt": "2", "sha": "a" * 40})
            with tarfile.open(runner / "chat-hub-transport.tar.gz") as archive:
                self.assertTrue(any(entry.name.endswith("native-receipt.json") for entry in archive))

    def test_ad_hoc_real_tauri_build(self):
        build = (ROOT / "build/build-native-macos.sh").read_text()
        config = (ROOT / "src-tauri/tauri.conf.json").read_text()
        self.assertIn("cargo tauri build", build)
        self.assertIn("--target aarch64-apple-darwin --bundles app -- --locked", build)
        self.assertNotIn("build --locked", build)
        self.assertIn("--bundles app", build)
        self.assertIn("codesign --force --deep --sign -", build)
        self.assertIn("codesign --verify --deep --strict", build)
        self.assertIn('"productName": "Chat Hub"', config)
        self.assertIn('"infoPlist": "Info.plist"', config)
        self.assertNotIn("notarized; ticket", build)

    def test_artifact_must_contain_real_outputs(self):
        workflow = (ROOT / ".github/workflows/macos.yml").read_text()
        build = (ROOT / "ci/actions/macos-native-build/action.yml").read_text()
        self.assertIn("runs-on: macos-15", workflow)
        self.assertIn("name: chat-hub-macos-arm64", workflow)
        self.assertIn("uses: ./ci/actions/gha-artifact-upload", workflow)
        self.assertIn("artifact-transfer:", workflow)
        self.assertIn('test "$TRANSFER_RESULT" = success', workflow)
        for entry in ("Chat Hub.app", "Chat_Hub_*.dmg", "verify-macos.txt"):
            self.assertIn(entry, build)

    def test_node_free_artifact_transfer_keeps_pr_unprivileged_and_main_fail_closed(self):
        workflow = (ROOT / ".github/workflows/macos.yml").read_text()
        pr = workflow.split("  package-pr:\n", 1)[1].split("  package-main:\n", 1)[0]
        main = workflow.split("  package-main:\n", 1)[1].split("  artifact-transfer:\n", 1)[0]
        gate = workflow.split("  native-release-gate:\n", 1)[1]
        self.assertIn("permissions:\n      contents: read", pr)
        self.assertNotIn("packages: write", pr)
        self.assertNotIn("GH_TOKEN:", pr)
        self.assertNotIn("gha-artifact-upload", pr)
        self.assertIn("publish-transport: 'false'", pr)
        self.assertIn("github.event_name == 'workflow_dispatch' && github.ref == 'refs/heads/main'", main)
        self.assertIn("packages: write", main)
        self.assertIn("publish-transport: 'true'", main)
        self.assertIn('test "$TRANSFER_RESULT" = success', gate)
        self.assertIn('test "$TRANSFER_RESULT" = skipped', gate)

    def test_main_acceptance_rebuilds_exact_sha_and_requires_transfer_before_mapping(self):
        workflow = (ROOT / ".github/workflows/accept-native.yml").read_text()
        self.assertIn('branches: [main]', workflow)
        native = workflow.split("  native-build:\n", 1)[1].split("  artifact-transfer:\n", 1)[0]
        transfer = workflow.split("  artifact-transfer:\n", 1)[1].split("  map:\n", 1)[0]
        mapping = workflow.split("  map:\n", 1)[1]
        self.assertIn("packages: write", native)
        self.assertIn('checkout --quiet --detach "$GITHUB_SHA"', native)
        self.assertIn('if: needs.native-build.result == \'success\'', transfer)
        self.assertIn("chat-hub-native-release-main-${{ github.run_id }}-${{ github.run_attempt }}", transfer)
        self.assertIn("needs: [native-build, artifact-transfer]", mapping)
        self.assertIn('--mapping-run-id "$GITHUB_RUN_ID"', mapping)

    def test_native_actions_trigger_main_acceptance_and_ship_in_corresponding_source(self):
        native_scope = (ROOT / "build/native_scope.py").read_text()
        acceptance = (ROOT / ".github/workflows/accept-native.yml").read_text()
        sources = (ROOT / "build/release_sources.py").read_text()
        self.assertIn("'ci/actions/**'", native_scope)
        self.assertIn('"ci/actions/**"', acceptance)
        self.assertIn('"ci/actions"', sources)
        self.assertIn('".github/workflows"', sources)

    def test_workflow_checkouts_fetch_the_immutable_event_sha(self):
        files = [*ROOT.joinpath(".github/workflows").glob("*.yml"),
                 *ROOT.joinpath("ci/actions").rglob("*.yml")]
        for path in files:
            with self.subTest(path=path.relative_to(ROOT)):
                content = path.read_text()
                self.assertNotIn('fetch --no-tags origin "$GITHUB_REF"', content)
                if 'checkout --quiet --detach "$GITHUB_SHA"' in content:
                    self.assertIn('fetch --no-tags origin "$GITHUB_SHA"', content)

    def test_protected_publish_stays_separate(self):
        workflow = (ROOT / ".github/workflows/release.yml").read_text()
        self.assertIn("default: false", workflow)
        self.assertIn("if: github.ref == 'refs/heads/main' && inputs.publish", workflow)
        self.assertIn("name: release-review-${{ inputs.tag }}", workflow)
        self.assertNotIn("APPLE_DEVELOPER_ID", workflow)


if __name__ == "__main__":
    unittest.main()
