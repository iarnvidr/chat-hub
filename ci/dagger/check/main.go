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
	noNodePath  = "/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin"
	nodeGuard   = "if command -v node >/dev/null 2>&1; then echo 'Node executable is prohibited in this CI environment' >&2; exit 1; fi"
	rustCacheState = "/tmp/chat-hub-sccache-state"
	rustCacheDir   = "/tmp/chat-hub-sccache-cache"
	rustWrapper    = "/tmp/chat-hub-sccache-state/bin/rustc-cache-wrapper"
	rustTarget     = "x86_64-unknown-linux-gnu"
	rustPlatform   = "linux-rust-1.99.1-bookworm-x86_64"
)

// Checks keeps the original no-cache entrypoint available for manual CI calls.
func (m *ChatHubCi) Checks(ctx context.Context, source *dagger.Directory) (string, error) {
	return m.checks(ctx, source, "off", nil, "", "", "", "", "", "", "", false)
}

// CachedChecks runs the authoritative Linux graph and uses the trusted OCI cache.
func (m *ChatHubCi) CachedChecks(ctx context.Context, source *dagger.Directory, cacheMode string,
	registryToken *dagger.Secret, repository, eventName, ref, actor, runID, runAttempt, sha string) (string, error) {
	return m.checks(ctx, source, cacheMode, registryToken, repository, eventName,
		ref, actor, runID, runAttempt, sha, true)
}

func (m *ChatHubCi) checks(ctx context.Context, source *dagger.Directory, cacheMode string,
	registryToken *dagger.Secret, repository, eventName, ref, actor, runID, runAttempt, sha string,
	verifyWarmReuse bool) (string, error) {
	tasks := []struct {
		name string
		run  func(context.Context, *dagger.Directory) (string, error)
	}{{"contracts", m.Contracts}, {"rust-bun", func(ctx context.Context, source *dagger.Directory) (string, error) {
		return m.linuxChecks(ctx, source, cacheMode, registryToken, repository, eventName,
			ref, actor, runID, runAttempt, sha, verifyWarmReuse)
	}}, {"browser", m.BrowserChecks}}
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

// SeedLinuxCache runs trusted-main Rust checks and verifies a real repeated
// compiler hit before writing the Linux cache to GHCR.
func (m *ChatHubCi) SeedLinuxCache(ctx context.Context, source *dagger.Directory,
	registryToken *dagger.Secret, repository, eventName, ref, actor, runID, runAttempt, sha string) (string, error) {
	return m.linuxChecks(ctx, source, "publish", registryToken, repository,
		eventName, ref, actor, runID, runAttempt, sha, true)
}

func (m *ChatHubCi) BrowserChecks(ctx context.Context, source *dagger.Directory) (string, error) {
	return dag.Container().From(bunImage).
		WithEnvVariable("PATH", noNodePath).
		WithExec([]string{"sh", "-ec", nodeGuard}).
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
		WithExec([]string{"bun", "tests/desktop_chat_features.cjs"}).
		WithExec([]string{"bun", "tests/desktop_ui_graph_browser.cjs"}).Stdout(ctx)
}

func (m *ChatHubCi) Contracts(ctx context.Context, source *dagger.Directory) (string, error) {
	python := dag.Container().From(pythonImage)
	bun := dag.Container().From(bunImage)
	rust := dag.Container().From(rustImage)
	return python.
		WithEnvVariable("PATH", "/usr/local/cargo/bin:"+noNodePath).
		WithExec([]string{"sh", "-ec", nodeGuard}).
		WithExec([]string{"sh", "-ec", "apt-get update && apt-get install -y --no-install-recommends bash build-essential git && rm -rf /var/lib/apt/lists/*"}).
		WithFile("/usr/local/bin/bun", bun.File("/usr/local/bin/bun")).
		WithDirectory("/usr/local/cargo", rust.Directory("/usr/local/cargo")).
		WithDirectory("/usr/local/rustup", rust.Directory("/usr/local/rustup")).
		WithEnvVariable("CARGO_HOME", "/usr/local/cargo").
		WithEnvVariable("RUSTUP_HOME", "/usr/local/rustup").
		WithEnvVariable("PATH", "/usr/local/cargo/bin:"+noNodePath).
		WithDirectory("/src", source).WithWorkdir("/src").
		WithExec([]string{"python3", "-m", "unittest", "discover", "-s", "tests", "-p", "test_*.py"}).
		WithExec([]string{"python3", "-m", "unittest", "discover", "-s", "build", "-p", "test_*.py"}).Stdout(ctx)
}

// LinuxChecks keeps the legacy manual CI command available without registry access.
func (m *ChatHubCi) LinuxChecks(ctx context.Context, source *dagger.Directory) (string, error) {
	return m.linuxChecks(ctx, source, "off", nil, "", "", "", "", "", "", "", false)
}

func (m *ChatHubCi) linuxChecks(ctx context.Context, source *dagger.Directory, cacheMode string,
	registryToken *dagger.Secret, repository, eventName, ref, actor, runID, runAttempt, sha string,
	verifyWarmReuse bool) (string, error) {
	var out strings.Builder
	bun := dag.Container().From(bunImage)
	rust := dag.Container().From(rustImage).
		WithEnvVariable("PATH", "/usr/local/cargo/bin:"+noNodePath).
		WithExec([]string{"sh", "-ec", nodeGuard}).
		WithExec([]string{"sh", "-ec", "apt-get update && apt-get install -y --no-install-recommends build-essential pkg-config python3 libssl-dev libxdo-dev libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev patchelf ffmpeg libatomic1 && rm -rf /var/lib/apt/lists/*"}).
		WithExec([]string{"rustup", "component", "add", "rustfmt", "clippy"}).
		WithFile("/usr/local/bin/bun", bun.File("/usr/local/bin/bun")).
		WithDirectory("/src", source).WithWorkdir("/src").
		WithEnvVariable("GITHUB_REPOSITORY", repository).
		WithEnvVariable("GITHUB_EVENT_NAME", eventName).
		WithEnvVariable("GITHUB_REF", ref).
		WithEnvVariable("GITHUB_ACTOR", actor).
		WithEnvVariable("GITHUB_RUN_ID", runID).
		WithEnvVariable("GITHUB_RUN_ATTEMPT", runAttempt).
		WithEnvVariable("GITHUB_SHA", sha)

	if cacheMode != "off" {
		rust = rust.WithEnvVariable("SCCACHE_DIR", rustCacheDir).
			WithEnvVariable("SCCACHE_CACHE_SIZE", "1G").
			WithEnvVariable("CARGO_INCREMENTAL", "0").
			WithExec([]string{"python3", "ci/scripts/compiled_cache.py", "prepare", "--root", "/src",
				"--platform", "linux-x86_64", "--platform-id", rustPlatform, "--target", rustTarget,
				"--cache-dir", rustCacheDir, "--state-dir", rustCacheState, "--repository", repository})
	}

	if cacheMode == "read" || cacheMode == "publish" {
		if registryToken == nil {
			return "", fmt.Errorf("compiled cache mode requires a scoped registry token")
		}
		rust = rust.WithSecretVariable("GH_TOKEN", registryToken).
			WithExec([]string{"python3", "ci/scripts/compiled_cache.py", "pull", "--state-dir", rustCacheState}).
			WithoutSecretVariable("GH_TOKEN").
			WithoutEnvVariable("GH_TOKEN")
	}
	if cacheMode != "off" {
		rust = rust.WithEnvVariable("RUSTC_WRAPPER", rustWrapper).
			WithEnvVariable("SCCACHE_CACHE_SIZE", "1G").
			WithEnvVariable("CARGO_INCREMENTAL", "0")
	}
	rust = rust.
		WithExec([]string{"sh", "-ec", "test -z \"${GH_TOKEN+x}\""}).
		WithExec([]string{"cargo", "fmt", "--all", "--", "--check"}).
		WithExec([]string{"cargo", "clippy", "--locked", "--all-targets", "--all-features", "--", "-D", "warnings"}).
		WithExec([]string{"cargo", "test", "--locked", "--all-targets"})
	if verifyWarmReuse {
		rust = rust.WithExec([]string{"python3", "ci/scripts/compiled_cache.py", "warm-probe",
			"--state-dir", rustCacheState, "--root", "/src"})
	}
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
	if cacheMode == "publish" {
		if registryToken == nil {
			return "", fmt.Errorf("cache publish requires a scoped registry token")
		}
		published, err := rust.WithSecretVariable("GH_TOKEN", registryToken).
			WithExec([]string{"python3", "ci/scripts/compiled_cache.py", "publish", "--state-dir", rustCacheState}).
			WithoutSecretVariable("GH_TOKEN").
			WithoutEnvVariable("GH_TOKEN").Stdout(ctx)
		if err != nil {
			return "", fmt.Errorf("publish Rust compiler cache: %w", err)
		}
		out.WriteString(published)
	} else if cacheMode == "read" {
		stats, err := rust.WithExec([]string{"python3", "ci/scripts/compiled_cache.py", "report", "--state-dir", rustCacheState}).Stdout(ctx)
		if err != nil {
			return "", fmt.Errorf("report Rust compiler cache: %w", err)
		}
		out.WriteString(stats)
	}
	return out.String(), nil
}
