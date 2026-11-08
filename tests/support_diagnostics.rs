// Run the pure native report/redaction and atomic-write regressions in Linux CI,
// without compiling Tauri or opening a dialog. The macOS gate validates wiring.
#[path = "../src-tauri/src/diagnostics_snapshot.rs"]
mod diagnostics_snapshot;
