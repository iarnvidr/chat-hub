use std::sync::{
    atomic::{AtomicBool, Ordering},
    OnceLock,
};

static IN_FLIGHT: OnceLock<AtomicBool> = OnceLock::new();

pub struct Permit(&'static AtomicBool);
impl Drop for Permit {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

pub fn try_acquire() -> Result<Permit, &'static str> {
    let flag = IN_FLIGHT.get_or_init(|| AtomicBool::new(false));
    flag.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
        .map(|_| Permit(flag))
        .map_err(|_| "archive_busy")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_overlap_and_releases_on_drop() {
        let permit = try_acquire().unwrap();
        assert_eq!(try_acquire().err(), Some("archive_busy"));
        drop(permit);
        assert!(try_acquire().is_ok());
    }
}
