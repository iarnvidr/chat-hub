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
    unsafe fn text(object: Id, selector: &[u8]) -> String {
        if object.is_null() {
            return String::new();
        }
        let send: unsafe extern "C" fn(Id, Sel) -> Id =
            std::mem::transmute(objc_msgSend as *const ());
        let value = send(object, sel_registerName(selector.as_ptr().cast()));
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
        let name = text(exception, b"name\0");
        let reason = text(exception, b"reason\0");
        let stack = text(exception, b"callStackSymbols\0");
        let report = format!("Uncaught Objective-C exception\nname: {name}\nreason: {reason}\ncallStackSymbols: {stack}\n");
        let _ = std::io::stderr().write_all(report.as_bytes());
        if let Some(path) = std::env::var_os("CHAT_HUB_CRASH_EVIDENCE") {
            if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
                let _ = file.write_all(report.as_bytes());
            }
        }
    }
    pub fn install() {
        unsafe {
            NSSetUncaughtExceptionHandler(Some(log));
        }
    }
}
#[cfg(target_os = "macos")]
pub fn install() {
    macos::install();
}
#[cfg(not(target_os = "macos"))]
pub fn install() {}
