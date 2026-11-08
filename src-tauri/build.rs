use std::{env, fs, path::Path, process::Command};

fn git(root: &Path, args: &[&str]) -> Option<String> {
    let result = Command::new("git")
        .current_dir(root)
        .args(args)
        .output()
        .ok()?;
    result
        .status
        .success()
        .then(|| String::from_utf8_lossy(&result.stdout).trim().to_owned())
}

fn object_id(root: &Path, reference: &str) -> Option<String> {
    git(root, &["rev-parse", "--verify", reference]).filter(|value| {
        value.len() == 40
            && value
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}

fn main() {
    let manifest = env::var_os("CARGO_MANIFEST_DIR").expect("Cargo manifest directory");
    let root = Path::new(&manifest).parent().expect("workspace root");
    // Rebuild only when identity changes, not on every invocation. Worktrees and
    // detached CI checkouts have different Git administrative directories.
    let mut refs = vec!["HEAD".to_owned(), "packed-refs".to_owned()];
    if let Some(reference) = git(root, &["symbolic-ref", "-q", "HEAD"]) {
        refs.push(reference);
    }
    for reference in refs {
        if let Some(path) = git(root, &["rev-parse", "--git-path", &reference]) {
            let path = root.join(path);
            if path.is_file() {
                println!("cargo:rerun-if-changed={}", path.display());
            }
        }
    }
    let commit = object_id(root, "HEAD");
    let tree = object_id(root, "HEAD^{tree}");
    let quoted = |value: Option<&str>| {
        value
            .map(|v| format!("\"{v}\""))
            .unwrap_or_else(|| "null".into())
    };
    let version = env::var("CARGO_PKG_VERSION").expect("Cargo package version");
    assert!(version
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b".-+".contains(&b)));
    let json = format!(
        "{{\"schema\":1,\"app_version\":\"{version}\",\"source_commit\":{},\"source_tree\":{}}}\n",
        quoted(commit.as_deref()),
        quoted(tree.as_deref())
    );
    if let Some(commit) = commit {
        println!("cargo:rustc-env=CHAT_HUB_BUILD_COMMIT={commit}");
    }
    // Tauri embeds frontendDist into the native binary. This ignored generated
    // file is not part of the Git source tree used by acceptance receipts.
    let target = root.join("desktop-ui/build-identity.json");
    if fs::read_to_string(&target).ok().as_deref() != Some(json.as_str()) {
        fs::write(target, json).expect("write embedded build identity");
    }
    tauri_build::build()
}
