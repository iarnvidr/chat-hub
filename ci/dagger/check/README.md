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

The Actions job can seed the engine from compatible GHCR OCI cache data, or
compile cold when no previous cache is available. Read-only restore happens in
the unprivileged pull-request job. A trusted `workflow_run` publisher separately
validates run identity and publishes inert cache data and successful evidence.
That publisher becomes active only after this workflow lands on `main`.

Run local source-closure and stable fingerprint regressions with
`python3 -m unittest discover -s ci/dagger/check -p 'test_*.py' -v`.
