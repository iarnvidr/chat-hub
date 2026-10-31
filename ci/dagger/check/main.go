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
	pythonImage     = "python:3.14.0-slim-bookworm@sha256:d13fa0424035d290decef3d575cea23d1b7d5952cdf429df8f5542c71e961576"
	rustImage       = "rust:1.99.1-bookworm@sha256:9181b56c2a21ab1d5e73cffe04fc762426d30aa7fac3c91141dd033f49a7fc8b"
	nodeImage       = "node:22.23.3-bookworm-slim@sha256:3ed98443660ff48413b5af19ecefce3b9d50845a901a43420d47cc63a9fddbe9"
	playwrightImage = "mcr.microsoft.com/playwright:v1.58.2-noble@sha256:6446946a1d9fd62d9ae501312a2d76a43ee688542b21622056a372959b65d63d"
)

// Checks is the single authoritative Linux graph, including real browser checks.
func (m *ChatHubCi) Checks(ctx context.Context, source *dagger.Directory) (string, error) {
	tasks := []struct {
		name string
		run  func(context.Context, *dagger.Directory) (string, error)
	}{{"contracts", m.Contracts}, {"rust-node", m.LinuxChecks}, {"browser", m.BrowserChecks}}
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
	return dag.Container().From(playwrightImage).
		WithDirectory("/src", source).WithWorkdir("/src").
		WithMountedCache("/root/.npm", dag.CacheVolume("chat-hub-browser-npm")).
		WithExec([]string{"npm", "install", "--prefix", "/opt/playwright", "--no-audit", "--no-fund", "playwright@1.58.2"}).
		WithEnvVariable("PLAYWRIGHT_MODULE", "/opt/playwright/node_modules/playwright").
		WithExec([]string{"node", "tests/desktop_ui_browser.cjs"}).Stdout(ctx)
}

func (m *ChatHubCi) Contracts(ctx context.Context, source *dagger.Directory) (string, error) {
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

func (m *ChatHubCi) LinuxChecks(ctx context.Context, source *dagger.Directory) (string, error) {
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
