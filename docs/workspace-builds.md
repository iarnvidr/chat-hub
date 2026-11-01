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
