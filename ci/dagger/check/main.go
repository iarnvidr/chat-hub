package main

import (
	"context"
	"errors"
	"fmt"
	"strings"
	"sync"

	"dagger/chat-hub-ci/internal/dagger"
)

type ChatHubCi struct{}

const (
	pythonImage = "python:3.14.0-slim-bookworm@sha256:d13fa0424035d290decef3d575cea23d1b7d5952cdf429df8f5542c71e961576"
	rustImage   = "rust:1.99.1-bookworm@sha256:9181b56c2a21ab1d5e73cffe04fc762426d30aa7fac3c91141dd033f49a7fc8b"
	bunImage    = "oven/bun:1.2.22@sha256:66ba69deede44e3af5dc542def218fdb3bcad2205900ea761dc5623bf973d2df"
)

// Checks is the single authoritative Linux graph, including real browser checks.
func (m *ChatHubCi) Checks(ctx context.Context, source *dagger.Directory) (string, error) {
	tasks := []struct {
		name string
		run  func(context.Context, *dagger.Directory) (string, error)
	}{{"contracts", m.Contracts}, {"rust-bun", m.LinuxChecks}, {"browser", m.BrowserChecks}}
	results := make([]string, len(tasks))
	failures := make([]error, len(tasks))
	var workers sync.WaitGroup
	for i, task := range tasks {
		workers.Add(1)
		go func(i int, name string, run func(context.Context, *dagger.Directory) (string, error)) {
			defer workers.Done()
			text, err := run(ctx, source)
			if err != nil {
				failures[i] = fmt.Errorf("%s: %w", name, err)
				return
			}
			results[i] = name + ": " + text
		}(i, task.name, task.run)
	}
	workers.Wait()
	if err := errors.Join(failures...); err != nil {
		return "", err
	}
	return strings.Join(results, "\n"), nil
}

func (m *ChatHubCi) BrowserChecks(ctx context.Context, source *dagger.Directory) (string, error) {
	return dag.Container().From(bunImage).
		WithExec([]string{"sh", "-ec", "apt-get update && apt-get install -y --no-install-recommends chromium && rm -rf /var/lib/apt/lists/*"}).
		WithNewFile("/tmp/pwdeps/package.json", `{"private":true,"dependencies":{"playwright":"1.58.2"}}`).
		WithWorkdir("/tmp/pwdeps").
		WithMountedCache("/root/.bun/install/cache", dag.CacheVolume("chat-hub-bun-playwright")).
		WithExec([]string{"bun", "install", "--ignore-scripts"}).
		WithDirectory("/src", source).WithWorkdir("/src").
		WithEnvVariable("PLAYWRIGHT_MODULE", "/tmp/pwdeps/node_modules/playwright").
		WithEnvVariable("CHROMIUM_PATH", "/usr/bin/chromium").
		WithExec([]string{"bun", "tests/desktop_ui_browser.cjs"}).
		WithExec([]string{"bun", "tests/desktop_workspace.cjs"}).
		WithExec([]string{"bun", "tests/desktop_chat_features.cjs"}).Stdout(ctx)
}

func (m *ChatHubCi) Contracts(ctx context.Context, source *dagger.Directory) (string, error) {
	python := dag.Container().From(pythonImage)
	rust := dag.Container().From(rustImage)
	return python.
		WithExec([]string{"sh", "-ec", "apt-get update && apt-get install -y --no-install-recommends bash build-essential git && rm -rf /var/lib/apt/lists/*"}).
		WithDirectory("/usr/local/cargo", rust.Directory("/usr/local/cargo")).
		WithDirectory("/usr/local/rustup", rust.Directory("/usr/local/rustup")).
		WithEnvVariable("CARGO_HOME", "/usr/local/cargo").
		WithEnvVariable("RUSTUP_HOME", "/usr/local/rustup").
		WithEnvVariable("PATH", "/usr/local/cargo/bin:/usr/local/bin:/usr/bin:/bin").
		WithDirectory("/src", source).WithWorkdir("/src").
		WithExec([]string{"python3", "-m", "unittest", "discover", "-s", "tests", "-p", "test_*.py"}).
		WithExec([]string{"python3", "-m", "unittest", "discover", "-s", "build", "-p", "test_*.py"}).Stdout(ctx)
}

func (m *ChatHubCi) LinuxChecks(ctx context.Context, source *dagger.Directory) (string, error) {
	var out strings.Builder
	bun := dag.Container().From(bunImage)
	rust := dag.Container().From(rustImage).
		WithExec([]string{"sh", "-ec", "apt-get update && apt-get install -y --no-install-recommends build-essential pkg-config libssl-dev libxdo-dev libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev patchelf ffmpeg libatomic1 && rm -rf /var/lib/apt/lists/*"}).
		WithExec([]string{"rustup", "component", "add", "rustfmt", "clippy"}).
		WithFile("/usr/local/bin/bun", bun.File("/usr/local/bin/bun")).
		WithDirectory("/src", source).WithWorkdir("/src").
		WithExec([]string{"cargo", "fmt", "--all", "--", "--check"}).
		WithExec([]string{"cargo", "clippy", "--locked", "--all-targets", "--all-features", "--", "-D", "warnings"}).
		WithExec([]string{"cargo", "test", "--locked", "--all-targets"})
	text, err := rust.Stdout(ctx)
	if err != nil {
		return "", fmt.Errorf("Rust lint/tests: %w", err)
	}
	out.WriteString(text)

	bunTests := rust.WithExec([]string{"bash", "build/test-desktop-js.sh"})
	text, err = bunTests.Stdout(ctx)
	if err != nil {
		return "", fmt.Errorf("desktop JS tests under Bun: %w", err)
	}
	out.WriteString(text)
	return out.String(), nil
}
