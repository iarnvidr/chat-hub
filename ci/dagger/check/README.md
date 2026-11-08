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
objects and Cargo fingerprints belonging to **each of the nine crates**, with
one digest-bound `crate.tar.zst` and `crate-manifest.json` bundle per package.
They appear individually in the GitHub Actions run under
`chat-hub-crate-<package>-<run>-<attempt>`. The trusted `workflow_run` publisher
also makes immutable OCI packages available at
`ghcr.io/iarnvidr/chat-hub-crate-<package>:run-<run>-<attempt>`; moving `pr-N`
and green `main` aliases are added only after validation by trusted code.
An ordinary `oras pull` retrieves a bundle for examination or manual loading.
These **are compiled Cargo objects, not installable binaries or release
qualifications**. The CLI check additionally compiles a runnable Linux debug
binary, included in its own crate bundle. The macOS PR workflow retains a
separate signed (ad-hoc), unnotarized application ZIP and matching DMG for
hands-on testing.

For reliable incremental compilation, restore the **complete cache archive**
instead of overlaying individual crate objects: Cargo needs the corresponding
dependency outputs, fingerprints and stable source mirrors. The per-crate
packages provide separately addressable objects while the shared snapshot
remains the dependency-coherent restore unit. Compiler output is never trusted
as a substitute for the current commit's green checks.

The Actions job can seed the engine from compatible GHCR OCI cache data, or
compile cold when no previous cache is available. Read-only restore happens in
the unprivileged pull-request job. A trusted `workflow_run` publisher separately
validates run identity and publishes inert cache data and successful evidence.
That publisher becomes active only after this workflow lands on `main`.

Run local source-closure and stable fingerprint regressions with
`python3 -m unittest discover -s ci/dagger/check -p 'test_*.py' -v`.
