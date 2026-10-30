package main

import (
	"context"
	"fmt"
	"strings"

	"dagger/check/internal/dagger"
)

type ChatHub struct{}

const (
	pythonImage = "python:3.14.0-slim-bookworm@sha256:d13fa0424035d290decef3d575cea23d1b7d5952cdf429df8f5542c71e961576"
	rustImage   = "rust:1.99.1-bookworm@sha256:9181b56c2a21ab1d5e73cffe04fc762426d30aa7fac3c91141dd033f49a7fc8b"
	nodeImage   = "node:22.23.3-bookworm-slim@sha256:3ed98443660ff48413b5af19ecefce3b9d50845a901a43420d47cc63a9fddbe9"
)

func (m *ChatHub) Contracts(ctx context.Context, source *dagger.Directory) (string, error) {
	python := dag.Container().From(pythonImage)
	rust := dag.Container().From(rustImage)
	node := dag.Container().From(nodeImage)
	return python.
		WithExec([]string{"sh", "-ec", "apt-get update && apt-get install -y --no-install-recommends bash git && rm -rf /var/lib/apt/lists/*"}).
		WithDirectory("/usr/local/cargo", rust.Directory("/usr/local/cargo")).
		WithDirectory("/usr/local/rustup", rust.Directory("/usr/local/rustup")).
		WithFile("/usr/local/bin/node", node.File("/usr/local/bin/node")).
		WithEnvVariable("CARGO_HOME", "/usr/local/cargo").
		WithEnvVariable("RUSTUP_HOME", "/usr/local/rustup").
		WithEnvVariable("PATH", "/usr/local/cargo/bin:/usr/local/bin:/usr/bin:/bin").
		WithDirectory("/src", source).WithWorkdir("/src").
		WithExec([]string{"python3", "-m", "unittest", "discover", "-s", "tests", "-p", "test_*.py"}).
		WithExec([]string{"python3", "-m", "unittest", "discover", "-s", "build", "-p", "test_*.py"}).Stdout(ctx)
}

func (m *ChatHub) LinuxChecks(ctx context.Context, source *dagger.Directory) (string, error) {
	var out strings.Builder
	rust := dag.Container().From(rustImage).
		WithExec([]string{"sh", "-ec", "apt-get update && apt-get install -y --no-install-recommends build-essential pkg-config libssl-dev libxdo-dev libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev patchelf ffmpeg && rm -rf /var/lib/apt/lists/*"}).
		WithDirectory("/src", source).WithWorkdir("/src").
		WithExec([]string{"cargo", "fmt", "--all", "--", "--check"}).
		WithExec([]string{"cargo", "clippy", "--locked", "--all-targets", "--all-features", "--", "-D", "warnings"}).
		WithExec([]string{"cargo", "test", "--locked", "--all-targets"})
	text, err := rust.Stdout(ctx)
	if err != nil {
		return "", fmt.Errorf("Rust lint/tests: %w", err)
	}
	out.WriteString(text)

	node := dag.Container().From(nodeImage).
		WithDirectory("/src", source).WithWorkdir("/src").
		WithExec([]string{"node", "--test", "tests/desktop_ui_*.test.cjs"})
	text, err = node.Stdout(ctx)
	if err != nil {
		return "", fmt.Errorf("desktop UI tests: %w", err)
	}
	out.WriteString(text)
	return out.String(), nil
}
