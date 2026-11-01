# Chat Hub Linux contract checks

This Dagger 0.21.11 module runs the repository's Python contract suite and build-helper tests in one Linux container.
All GitHub Actions compiler jobs use the protected-main read-only compiler cache; main-only cache
writers publish after a repeated leaf-crate build proves an sccache hit. The manual CI workflow uses
`cached-linux-checks`. The legacy Dagger `checks --source .` and `linux-checks --source .` entrypoints
remain available for compatibility and run uncached.
The dedicated package keeps at most six versions after cleanup: the newest Linux/macOS
entries plus recent fallbacks. Concurrent platform publishers may briefly add extra versions;
each publish prunes again, and an uncertain listing or failed preflight deletion blocks that
cache push rather than allowing history to keep growing. Cache retention never gates builds.
