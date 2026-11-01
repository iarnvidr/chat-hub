# Workspace and compiled-output reuse

Chat Hub uses one Cargo workspace, one root `Cargo.lock`, and one shared `target` directory. The desktop remains a member with its own app manifest. There is no desktop-specific lockfile or second default build tree.

| Package | Responsibility | Versioning |
| --- | --- | --- |
| `chat-hub-model` | Conversation graph values, library snapshots and catalog DTOs | Independent `0.1.0` |
| `chat-hub-formats` | Bounded JSON export admission and pure transcript decoding | Independent `0.1.0` |
| `chat-hub` | SQLite persistence, indexing, capture, service and CLI | App version |
| `chat-hub-desktop` | Tauri desktop and provider-session integration | App version |

The leaf crates have no dependency on the database, service, network or desktop. Existing `chat_hub::store`, `chat_hub::catalog` and `chat_hub::importer` public paths re-export moved types/functions, so consumers keep their current API. Stable crate versions are not bumped merely because the desktop version changes.

The default workspace members are the core and leaf crates. Linux core checks cover those members; Apple Silicon validation also covers the desktop. Formatting covers the complete workspace. Native packaging resolves its bundle under the workspace target directory, including a caller-supplied `CARGO_TARGET_DIR`.

The workspace root owns the release profile. Its existing LTO, code-generation and stripping settings now apply to the desktop as well as the core; the hosted native package and smoke checks validate that resulting app.

All builds, tests and packaging for this work run in GitHub Actions. The source extraction does not change import limits, ZIP rejection, transcript roles, database schema or data locations. Corresponding-source bundles include the new crates, and native-change scope detection includes `crates/**`.

## Reuse expectations

Changing core service code leaves stable leaf source and versions unchanged. Compatible compiler outputs can be reused; a changed leaf invalidates its dependents. The final app still needs linking, and release optimization can do additional work.

Compiler caches and finished release artifacts serve different purposes. A cache speeds compilation and may be discarded or evicted; correctness checks must still pass when it is missing. The app/CLI artifacts and release evidence remain tied to their exact reviewed source and workflow run. A cached library is not evidence that a new source revision passed its required checks.

## GitHub Actions compiler cache

The hosted Linux and Apple Silicon builds use checksum-pinned sccache binaries. sccache stores compiler results rather than the complete Cargo target tree. Its object keys account for the compiler invocation, source, dependencies and build options. The outer cache bucket separates platform/SDK compatibility and does not invalidate all stable objects whenever the app version or lockfile changes.

Protected-main builds publish a dedicated compiler-cache package in GHCR. Pull requests have package-read permission and restore only that main-authored cache into a fresh job directory. Registry credentials use temporary isolated configuration and are removed before Cargo runs. No Node action or Node runtime is introduced.

Each runner's object cache is limited to 1 GiB. Publication checks the dedicated cache package's version capacity before pushing and targets six retained snapshots, protecting the newest Linux and macOS versions plus recent fallbacks. Concurrent publishers can briefly exceed that target; post-publication pruning removes extras. Uncertain listing or failed preflight pruning skips publication. Release packages and their evidence are outside this cleanup policy.

The first PR can have no compatible main cache yet. The Linux job therefore also builds the two leaf crates, removes their Cargo outputs and repeats that build, requiring an increase in Rust compiler-cache hits. This verifies actual reuse within hosted CI. Cross-run restore/publication becomes exercised after a protected-main seed; cache statistics are recorded in workflow output and native job summaries.
