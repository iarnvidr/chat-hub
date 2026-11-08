# Workspace and compiled-output reuse

Chat Hub uses one Cargo workspace and one root `Cargo.lock`. Its implementation
libraries are separate compilation units; the existing CLI remains the root
package and the desktop remains a workspace member. The desktop still embeds the
shared service in one app. This refactor does not introduce a remote-service
requirement or a second database owner.

## Ownership and dependency direction

| Package | Responsibility | Versioning |
| --- | --- | --- |
| `chat-hub-model` | Conversation/catalog values, capture observations and shared sink interfaces | Independent `0.1.0` |
| `chat-hub-formats` | Bounded format decoding and normalization without SQLite or native UI | Independent `0.1.0` |
| `chat-hub-store` | SQLite stores, migrations, catalog, backups and archive persistence | Independent `0.1.0` |
| `chat-hub-ingest` | Import admission and durable ingestion using formats and storage | Independent `0.1.0` |
| `chat-hub-capture` | TLS proxy, bounded stream capture and capture lifecycle | Independent `0.1.0` |
| `chat-hub-api` | Authenticated HTTP routes and service projections | App version; `/api/v1/version` reports the app version |
| `chat-hub-runtime` | Configuration, shared service lifetime and composition of adapters | Independent `0.1.0` |
| `chat-hub` | CLI and compatibility reexports, not implementation ownership | App version |
| `chat-hub-desktop` | Tauri windows, webviews, IPC and platform integration | App version |

The model and formats crates have no database, HTTP, runtime or Tauri dependency.
Capture delivers typed observations through a shared interface rather than
depending on SQLite. The runtime composes capture, ingestion, storage and HTTP;
implementation libraries do not import the root CLI/facade crate. Public
`chat_hub::store`, `chat_hub::importer`, `chat_hub::service` and related paths
re-export the real implementations instead of duplicating them.

The root owns the release profile, including LTO and stripping. A changed
library legitimately invalidates its dependent libraries and may require final
binary relinking; splitting crates does not promise that dependent executables
never rebuild. Unchanged independent libraries should remain fresh. The native
app path still respects `CARGO_TARGET_DIR`; there is no desktop-specific lockfile.

Data/security invariants remain: bounded imports and capture, explicit account
identity, the existing SQLite migrations/data paths, loopback authorization and
safe archive/updater handling. This is not a freeze on Chat Hub's unfinished UI.

## Dagger v1: independent checks and source closures

The root `dagger.toml` installs the native Dang module in `ci/dagger/check`.
GitHub pins Dagger **1.0.0-beta.25** and Rust **1.99.1**, with digest-pinned base
images. The module has a first-class check for every Cargo workspace package,
including the desktop, as well as formatting, Python, Bun and browser checks.
Each package check includes Clippy. The package-to-check coverage is regression
tested so adding a member cannot silently omit it from validation.

```sh
dagger check -l --all
dagger --progress logs check
```

Each Rust check runs Clippy and `cargo test --locked -p <package> --all-targets`.
Its source directory is the workspace skeleton plus that package's transitive local dependencies,
including test dependencies, included files and fixtures. The actual module and
its source-scope tests are authoritative for the graph. UI bytes are not inputs
to the independent model/format/store checks. A collection of named checks that
all mounted the entire checkout would not provide this isolation.

The root crate's `--all-targets` check owns the real parsed-URL/window-policy Rust
integration test. Python contract checks retain their independent `rustc`
policy/panic harnesses, but do not recursively launch a second Cargo build of
the root application. The manually dispatched workflow also reuses this graph
instead of running a duplicate headless Rust suite.

There are two different kinds of reuse:

1. A **Dagger operation cache hit** skips an operation whose actual input graph
   has already been evaluated on a reachable engine cache.
2. **Cargo `Fresh` output** means matching compiled artifacts were reused. Cargo
   can still execute tests even when it does not recompile a crate.

Neither a cache archive nor a `Fresh` line is a passing-check receipt for a new
candidate.

## Stable Cargo state, including source timestamps

Rust checks use the same `LOCKED` cache volume. The module's `run-rust.sh` and
`stable-worktree.py` perform a checksum-based source sync that
updates a stable worktree without copying fresh checkout timestamps onto
unchanged files. Cargo's downloaded registry/git data and compiled target tree
are retained alongside that mirror. Toolchain, target and native system-package
compatibility partition compiled outputs. Source symlinks and future-dated cache
artifacts are rejected rather than trusted as invisible inputs.

The shared lock and at most two Cargo build jobs limit memory pressure and keep
the mirror/target consistent. Independent check identities do not imply that all
Cargo commands execute simultaneously. Dagger still manages the graph and the
non-Rust checks independently.

## GitHub-hosted execution and GitHub Packages

`Checks / contracts` remains the required Linux job. PRs, main pushes and the
manual entrypoint use the same Dagger graph; the manual workflow calls that
workflow rather than copying the build commands. No private engine credentials
or Dagger Cloud account are required to validate a public PR.

A Dagger `CacheVolume` alone is not persistent across fresh GitHub-hosted VMs.
The Linux job therefore restores a compatible GHCR archive when available and
seeds the engine volume from it. A missing/incompatible cache takes the normal
cold-build path. Host-side restore uses a read-only package token; that token is
not passed into the Dagger compiler/check environment.

After checks, an `always()` export retains successful compilation even if a test
failed. The export nonce changes only the snapshot operation, never the compiler
volume's identity:

```sh
dagger -m ci/dagger/check api call export-cargo-cache \
  --nonce "<unique-export-id>" --output ci/cache/linux-cargo
```

Full Cargo logs are retained separately for failure diagnosis. Compiler data is
archived before upload so file timestamps and modes survive transport. A
separate `workflow_run` publisher runs only trusted default-branch code and
publishes inert data, never executes PR artifacts. Fork PRs do not publish and
PR checks never receive package-write permission. The existing Linux Go uploader
keeps this transport Node-free.

The publisher must exist on `main` before GitHub starts its `workflow_run`
trigger. Adding it to a draft PR is not evidence that a GHCR package has been
published. Initial PR checks can still run cold and retain their exported cache
as an Actions artifact.

| Retained object | Meaning |
| --- | --- |
| Cargo cache archive | Compilation/download optimization; may survive a failed test |
| Successful check receipt | Validation evidence bound to the exact tested candidate |
| Native app candidate and receipts | A platform-tested build, not automatically a signed production release |

These are distinct objects. A moving cache tag is not release authorization and
a commit label alone does not encode the PR merge base. The existing exact
candidate/tree checks and release promotion rules remain authoritative.

Apple Silicon keeps its separate checksum-pinned sccache path and real native
build/smoke jobs. Linux tests do not establish WKWebView, native launch, updater
or signing behavior. Protected-main native publication and release receipt
validation remain separate from the Linux cache publisher.

## Verification and transport cost

Measure cold, same-engine warm, fresh-runner restored, UI-only, runtime-only and
leaf-only changes, including a cache miss and a failing test followed by cache
recovery. Record Cargo `-vv` (`Fresh`/`Compiling`), timings, Dagger hits, cache
bytes and download/extract/export/upload time. A large archive can cost more
than the compilation it avoids; do not infer efficiency from cache existence.

Expected isolation includes frontend edits leaving backend libraries fresh,
store edits leaving capture libraries fresh, and a shared model edit rebuilding
the actual dependent packages. Source fixture/build-script changes must still
invalidate their consumers. Hosted traces and macOS acceptance remain required
before claiming full cross-run/native reuse or promoting an artifact.

Corresponding-source bundles include the crate workspace, Dagger module/config,
tests/fixtures and build helpers with their executable modes, but exclude
compiler snapshots under `ci/cache` and `target`. Native-change detection covers
these policy inputs and fixtures as well as the Rust/UI source.
