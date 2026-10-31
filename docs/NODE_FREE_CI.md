# Node-free CI artifacts

CI and release workflows do not run Node. The JavaScript test suites and Playwright browser fixtures run under Bun; native Tauri builds use the pinned Cargo Tauri CLI.

Pull requests still run the required macOS build and smoke checks for native changes. Pull request jobs have read-only repository permissions and do not publish a macOS bundle as a downloadable Actions artifact. This is an intentional limitation of the Node-free runner path: GitHub's maintained Actions artifact uploader requires Node, and the supported container-action path is available only on Linux.

After a same-repository pull request is merged to `main`, `accept-native.yml` rebuilds that exact main commit on macOS. The protected-main job sends the build through an immutable GHCR digest to a Linux Go uploader, which creates the normal GitHub Actions artifacts. The accepted mapping binds the bundle to the exact main commit, workflow run and attempt, and still requires the merged PR identity and successful Linux check receipt. Release retrieval does not fall back to a pull request artifact or infer that another build is equivalent.

Pull request builds do not receive package-write permissions. Trusted main and release workflows use those permissions only for the temporary OCI transport; the release environment and publication gates remain separate.
