//! Best-effort Objective-C uncaught-exception diagnostics for macOS.
#[cfg(target_os = "macos")]
mod macos {
    use std::{ffi::CStr, fs::OpenOptions, io::Write, os::raw::c_void};
    type Id = *mut c_void;
    type Sel = *mut c_void;
    unsafe extern "C" {
        fn NSSetUncaughtExceptionHandler(handler: Option<unsafe extern "C" fn(Id)>);
        fn sel_registerName(name: *const i8) -> Sel;
        fn objc_msgSend();
    }
    // Keep Objective-C object returns distinct from Rust-owned String values: objc_msgSend
    // returns an object pointer for these selectors, while UTF8String returns a C string.
    unsafe fn send_object(object: Id, selector: &[u8]) -> Id {
        if object.is_null() {
            return std::ptr::null_mut();
        }
        let send: unsafe extern "C" fn(Id, Sel) -> Id =
            std::mem::transmute(objc_msgSend as *const ());
        send(object, sel_registerName(selector.as_ptr().cast()))
    }
    unsafe fn string_utf8(value: Id) -> String {
        if value.is_null() {
            return String::new();
        }
        let utf8: unsafe extern "C" fn(Id, Sel) -> *const i8 =
            std::mem::transmute(objc_msgSend as *const ());
        let ptr = utf8(value, sel_registerName(b"UTF8String\0".as_ptr().cast()));
        if ptr.is_null() {
            String::new()
        } else {
            CStr::from_ptr(ptr).to_string_lossy().into_owned()
        }
    }
    unsafe extern "C" fn log(exception: Id) {
        // Never unwind across the Objective-C callback boundary. Diagnostics are best effort.
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let name = string_utf8(send_object(exception, b"name\0"));
            let reason = string_utf8(send_object(exception, b"reason\0"));
            // Preserve the original exception before doing any stack introspection, which may
            // itself fail. NSArray is not an NSString: render it through -description first.
            let initial =
                format!("Uncaught Objective-C exception\nname: {name}\nreason: {reason}\n");
            let _ = std::io::stderr().write_all(initial.as_bytes());
            if let Some(path) = std::env::var_os("CHAT_HUB_CRASH_EVIDENCE") {
                if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
                    let _ = file.write_all(initial.as_bytes());
                }
            }
            let stack_array = send_object(exception, b"callStackSymbols\0");
            let stack_description = send_object(stack_array, b"description\0");
            let stack = string_utf8(stack_description);
            let tail = format!("callStackSymbols: {stack}\n");
            let _ = std::io::stderr().write_all(tail.as_bytes());
            if let Some(path) = std::env::var_os("CHAT_HUB_CRASH_EVIDENCE") {
                if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
                    let _ = file.write_all(tail.as_bytes());
                }
            }
        }));
    }
    pub fn install() {
        unsafe {
            NSSetUncaughtExceptionHandler(Some(log));
        }
    }
}
fn bounded_line(text: &str) -> String {
    text.chars()
        .take(200)
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}
#[cfg(test)]
mod tests {
    use super::bounded_line;

    #[test]
    fn bounds_and_sanitizes_diagnostic_lines() {
        let input = format!("{}\nsecret", "x".repeat(300));
        let result = bounded_line(&input);
        assert_eq!(result.chars().count(), 200);
        assert!(!result.chars().any(char::is_control));
        assert!(bounded_line("safe").contains("safe"));
    }
}
fn panic_hook(info: &std::panic::PanicHookInfo<'_>) {
    let location = info
        .location()
        .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
        .unwrap_or_else(|| "unknown".into());
    let payload = info
        .payload()
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| info.payload().downcast_ref::<String>().map(String::as_str))
        .unwrap_or("non-string panic payload");
    let backtrace = std::backtrace::Backtrace::force_capture().to_string();
    let mut report = format!(
        "Rust panic at {}: {}\n",
        bounded_line(&location),
        bounded_line(payload)
    );
    for line in backtrace.lines().take(200) {
        report.push_str(&bounded_line(line));
        report.push('\n');
    }
    // Bound the complete diagnostic, not just individual lines, including evidence output.
    report = report
        .lines()
        .map(bounded_line)
        .collect::<Vec<_>>()
        .join("\n");
    report.push('\n');
    report.truncate(report.floor_char_boundary(40 * 1024));
    let _ = std::io::Write::write_all(&mut std::io::stderr(), report.as_bytes());
    if let Some(path) = std::env::var_os("CHAT_HUB_CRASH_EVIDENCE") {
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
        {
            let _ = std::io::Write::write_all(&mut file, report.as_bytes());
        }
    }
}
#[cfg(target_os = "macos")]
pub fn install() {
    std::panic::set_hook(Box::new(panic_hook));
    macos::install();
}
#[cfg(not(target_os = "macos"))]
pub fn install() {
    std::panic::set_hook(Box::new(panic_hook));
}
