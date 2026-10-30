//! Filesystem protocol for feature-gated native smoke requests.
use serde_json::Value;
use std::{
    fs, io,
    path::Path,
    sync::{
        atomic::{AtomicU64, Ordering},
        Mutex, OnceLock,
    },
};

static NEXT: AtomicU64 = AtomicU64::new(0);
static TERMINALS: OnceLock<Mutex<std::collections::HashSet<String>>> = OnceLock::new();

pub enum DispatchReply {
    Terminal(Value),
    Deferred,
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn publish(
    dir: &Path,
    id: &str,
    suffix: &str,
    label: &str,
    value: &Value,
    no_replace: bool,
) -> io::Result<()> {
    if !valid_id(id) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid request id",
        ));
    }
    let target = dir.join(format!("{id}.{suffix}"));
    let tmp = dir.join(format!(
        ".{id}.{label}.{}.{}.tmp",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let result = (|| {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)?;
        use io::Write;
        file.write_all(&serde_json::to_vec(value)?)?;
        file.sync_all()?;
        drop(file);
        if no_replace {
            match fs::hard_link(&tmp, &target) {
                Ok(()) => fs::remove_file(&tmp),
                Err(e) => Err(e),
            }
        } else {
            fs::rename(&tmp, &target)
        }
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

pub fn publish_terminal(dir: &Path, id: &str, value: &Value) -> io::Result<()> {
    if !valid_id(id) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid request id",
        ));
    }
    let key = format!("{}:{id}", dir.display());
    let terminals = TERMINALS.get_or_init(|| Mutex::new(std::collections::HashSet::new()));
    let mut seen = terminals.lock().unwrap_or_else(|p| p.into_inner());
    if !seen.insert(key.clone()) {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "terminal response already published",
        ));
    }
    drop(seen);
    match publish(dir, id, "res.json", "terminal", value, true) {
        Ok(()) => Ok(()),
        Err(e) => {
            let mut seen = terminals.lock().unwrap_or_else(|p| p.into_inner());
            seen.remove(&key);
            Err(e)
        }
    }
}

pub fn publish_dispatch(dir: &Path, id: &str, reply: DispatchReply) -> io::Result<()> {
    match reply {
        DispatchReply::Deferred => publish(
            dir,
            id,
            "ack.json",
            "ack",
            &serde_json::json!({"ok":true}),
            false,
        ),
        DispatchReply::Terminal(value) => publish_terminal(dir, id, &value),
    }
}
