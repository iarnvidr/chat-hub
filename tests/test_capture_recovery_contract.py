"""Source wiring checks; Rust/native behavioral acceptance remains separate."""
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[1]


class CaptureRecoveryWiringTests(unittest.TestCase):
    def test_live_and_legacy_writes_use_the_recovery_adapter(self):
        source = (ROOT / 'crates/chat-hub-store/src/capture.rs').read_text()
        self.assertEqual(source.count('self.store_captured_observation('), 2)
        journal = (ROOT / 'crates/chat-hub-store/src/capture_journal.rs').read_text()
        stage = journal.index('let ticket = journal.stage(observation)?;')
        commit = journal.index('let result = self.ingest_observation_outcome(observation);', stage)
        acknowledgement = journal.index('journal.acknowledge(&ticket)', commit)
        self.assertLess(stage, commit)
        self.assertLess(commit, acknowledgement)

    def test_startup_replays_before_service_writer_starts(self):
        source = (ROOT / 'src-tauri/src/desktop_runtime.rs').read_text()
        self.assertLess(source.index('store.recover_pending_captures()'), source.index('service::start_with_proxy('))
        self.assertIn('tokio::task::spawn_blocking', source)
        self.assertIn('capture_recovery', source)

    def test_export_is_native_menu_initiated_and_does_not_add_ipc(self):
        menu = (ROOT / 'src-tauri/src/desktop_menu.rs').read_text()
        native = (ROOT / 'src-tauri/src/support_diagnostics.rs').read_text()
        self.assertIn('support_diagnostics::export(app)', menu)
        self.assertIn('Export diagnostics', menu)
        self.assertIn('.save_file(', native)
        self.assertIn('snapshot::metadata_report(', native)
        self.assertIn('snapshot::write_report(', native)
        self.assertNotIn('#[tauri::command]', native)
        self.assertNotIn('.eval(', native)


if __name__ == '__main__':
    unittest.main()
