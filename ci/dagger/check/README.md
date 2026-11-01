# Chat Hub Linux contract checks

This Dagger 0.21.11 module runs the repository's Python contract suite and build-helper tests in one Linux container.
Run the Linux Rust checks locally with `dagger call -m ci/dagger/check linux-checks --source .`.
The original `checks --source .` and `linux-checks --source .` entrypoints remain available without
registry access. GitHub uses `cached-checks` with a read-only protected-main compiler cache; the
main-only `seed-linux-cache` entrypoint publishes only after a repeated leaf-crate build proves an
sccache hit.
The dedicated package keeps at most six versions after cleanup: the newest Linux/macOS
entries plus recent fallbacks. Concurrent platform publishers may briefly add extra versions;
each publish prunes again, and an uncertain listing or failed preflight deletion blocks that
cache push rather than allowing history to keep growing. Cache retention never gates builds.
