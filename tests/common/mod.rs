use std::{
    io::ErrorKind,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

/// Reserve a fresh directory; never reuse or remove another test's state.
pub fn temp_dir(label: &str) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    loop {
        let path = std::env::temp_dir().join(format!(
            "{label}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        match std::fs::create_dir(&path) {
            Ok(()) => return path,
            Err(error) if error.kind() == ErrorKind::AlreadyExists => continue,
            Err(error) => panic!("reserve isolated test directory: {error}"),
        }
    }
}
