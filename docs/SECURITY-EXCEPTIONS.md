# Temporary security exceptions

## GHSA-wrw7-89jp-8q8g / RUSTSEC-2024-0429 — glib

**Scope:** initial prerelease on macOS only. This is a platform-nonapplicability disposition, not acceptance for Linux desktop builds or for unknown advisories.

RustSec describes unsound `Iterator` / `DoubleEndedIterator` implementations for `glib::VariantStrIter`: an immutable pointer reference is passed where GLib mutates an out-argument, leading to undefined behavior and potentially null-pointer crashes. The affected range is glib 0.15.0 through versions before 0.20.0; glib 0.18.5 is affected. Advisory: https://rustsec.org/advisories/RUSTSEC-2024-0429.html (GHSA: https://github.com/advisories/GHSA-wrw7-89jp-8q8g).

Target-specific dependency-tree evidence for `src-tauri/Cargo.toml`:

- `cargo tree --manifest-path src-tauri/Cargo.toml --target aarch64-apple-darwin -i glib` reports “warning: nothing to print.” Thus the affected crate and its API are absent from the macOS target graph.
- The corresponding `x86_64-unknown-linux-gnu` tree includes glib 0.18.5 through GTK, WebKitGTK, and Tauri/Wry. Linux desktop is not shipped in this initial release.
- The companion core crate (`Cargo.toml`) has no GTK dependency; this exception does not cover it.

The advisory concerns use of glib's VariantStrIter, but on macOS neither glib nor the GTK stack is compiled into this app, so this affected code is unreachable in the shipped macOS target. No compatible patched version is available in the current dependency resolution. The dependency-review workflow therefore allows only this exact GHSA; all other advisories remain subject to normal review and blocking behavior.

**Expiry:** remove this exception and the workflow allow-list immediately after upgrading Tauri/dependencies so the affected glib version is no longer reported. Reassess rather than extending the exception. Linux desktop builds remain out of scope and must not ship under this disposition.
