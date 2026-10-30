# Desktop UI preview

The desktop UI has an explicit, development-only fixture adapter for browser review. From the repository root, serve this checkout with `python3 -m http.server 8000` and open `http://127.0.0.1:8000/desktop-ui/index.html?fixture=demo`. The page labels this mode **Development fixtures — native service not connected**; it never silently substitutes fixture data in the native app or ordinary browser view. Fixture mode exercises the reader, branches, search snippets, pagination, and retryable unavailable-service state; it does not demonstrate backend integration.

The native desktop application requires the Rust/Tauri service bridge. Backend integration and native macOS packaging/file-picker verification are outside this fixture preview; macOS build/release workflows are unchanged by this UI work.

Linux checks: `node --test tests/desktop_ui_*.test.cjs`; the CI workflow additionally runs the Playwright browser check using an isolated temporary install.
