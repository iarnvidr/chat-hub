# Chat Hub Dagger v1 checks

The root `dagger.toml` selects this Dang module (`dagger-module.toml`) on
Dagger `v1.0.0-beta.25`. GitHub Actions runs the checks with `dagger check`.

Each of the nine Rust workspace members has an independent `@check` entrypoint,
with narrow source directories matching its production and test dependencies.
Each check runs `cargo clippy --all-targets --all-features` and `cargo test
--all-targets`; the whole workspace also receives a non-compiling formatting
check. Separate Bun, Chromium, and Python contract checks run in their own
containers. Native macOS build verification remains in the macOS workflow.

Compilation reuses a single locked Cargo cache volume; source files are mirrored
into a stable worktree to keep unchanged Cargo fingerprints fresh. `run-rust.sh`
preserves complete per-check compiler logs. Cache export is a separate Dagger
function and is attempted even after failed validation: successful checks and
retained compilation artifacts are independent.

On successful runs, `ci/scripts/crate_artifacts.py` extracts the actual compiled
objects, Cargo fingerprints, and verified source timestamps belonging to **each of the nine crates**, with
one digest-bound `crate.tar.zst` and `crate-manifest.json` bundle per package.
They appear individually in the GitHub Actions run under
`chat-hub-crate-<package>-<run>-<attempt>`. The trusted `workflow_run` publisher
also makes immutable OCI packages available at
`ghcr.io/iarnvidr/chat-hub-crate-<package>:run-<run>-<attempt>`; moving `pr-N`
and green `main` aliases are added only after validation by trusted code.
These are actual reusable input packages: the pre-Dagger Actions step resolves
each `pr-N`/`main` OCI alias to an immutable digest, verifies the checksum,
trusted publisher, exact Dagger source closure, and pinned compiler recipe,
and restores only matching crate-owned target files and source timestamps.
Its SHA-256 source closure comes directly from the `main.dang` Directory
expressions, including transitive workspace dependencies and synthetic
placeholders; editing an unrelated crate therefore cannot invalidate a leaf
package. Independent misses are reported, and Cargo compiles those crates.
The shared snapshot still supplies third-party dependency objects efficiently,
and Cargo's own fingerprint logic remains the final authority on freshness.
An ordinary `oras pull` can also retrieve an immutable bundle for inspection.
These **are compiled Cargo objects, not installable binaries or release
qualifications**. The CLI check additionally compiles a runnable Linux debug
binary, included in its own crate bundle. The macOS PR workflow retains a
separate signed (ad-hoc), unnotarized application ZIP and matching DMG for
hands-on testing.

The warm path restores the complete cache snapshot **and** individually matches
and consumes the published crate packages. Even with no shared snapshot, a
standalone crate package can be loaded: it carries the owner objects and their
source timestamps. Cargo may still have to rebuild it if required third-party
dependencies are absent or compiler/linker configuration changed. Changes to a
library correctly rebuild its downstream dependents; unrelated libraries remain
reusable. Compiler output is never evidence for skipping current checks.

The Actions job can seed the engine from compatible GHCR OCI packages, or
compile cold when no previous package is available. Read-only restore happens in
the unprivileged pull-request job. A trusted `workflow_run` publisher separately
validates run identity and publishes inert cache data and successful evidence.
That publisher becomes active only after this workflow lands on `main`.

Tests verify OCI package selection and restoration, poisoned-archive rejection,
and real Cargo JSON `fresh: true` after restoring only one published-style
crate archive into a new cache. Run local source-closure and fingerprint regressions with
`python3 -m unittest discover -s ci/dagger/check -p 'test_*.py' -v`.
