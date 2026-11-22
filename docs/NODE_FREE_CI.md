# Node-free CI artifacts

CI and release workflows do not run Node. The JavaScript test suites and Playwright browser fixtures run under Bun; native Tauri builds use the pinned Cargo Tauri CLI.

Pull requests run the required Linux Dagger, security and UI checks, without launching Apple Silicon. The legacy `native-release-gate` required status is an Ubuntu-only compatibility check and explicitly does not claim native validation. Run `Apple Silicon build (on demand)` manually on a feature branch for an unprivileged macOS candidate, or on `main` for a corresponding-source artifact. The native candidate and release acceptance never run implicitly.

Only when manually dispatched on protected `main`, `accept-native.yml` builds the exact requested commit on macOS. The protected-main job sends the build through an immutable GHCR digest to a Linux Go uploader, which creates the normal GitHub Actions artifacts. The accepted mapping binds the bundle to the exact main commit, workflow run and attempt, and still requires the merged PR identity and successful Linux check receipt. Release retrieval does not fall back to a pull request artifact or infer that another build is equivalent.

Pull request builds do not receive package-write permissions. Linux checks use
the native Dagger v1 graph and restore compatible compiler data from GHCR with
read-only package access. Their cache export runs after failures as well as
successes and uses the Linux Go uploader, without turning a failed check into a
passing receipt.

The separate `publish-ci-cache.yml` workflow runs trusted default-branch code
after a same-repository `Checks` run. It validates and publishes the inert cache
archive to GHCR without executing pull request code. It can publish useful
compilation outputs from a failed test run, but cannot publish from fork PRs.
This compiler-cache publisher is distinct from the temporary native-app OCI
transport and grants no release authorization. The release environment, exact
candidate receipts and publication gates remain separate.
