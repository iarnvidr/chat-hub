//! App-scoped WKWebView trust for certificates issued by this installation's proxy CA.
use objc2::msg_send;
use objc2::runtime::AnyObject;
use security_framework::{certificate::SecCertificate, trust::SecTrust};
use std::{
    ffi::{c_char, c_void, CStr},
    sync::OnceLock,
};
use tauri::WebviewWindow;
static CA_DER: OnceLock<Vec<u8>> = OnceLock::new();
extern "C" {
    fn object_getClass(obj: *const c_void) -> *const c_void;
    fn sel_registerName(name: *const c_char) -> *mut c_void;
    fn class_addMethod(
        cls: *const c_void,
        name: *const c_void,
        imp: *const c_void,
        types: *const c_char,
    ) -> bool;
    fn objc_getClass(name: *const c_char) -> *const c_void;
}
type Completion = block2::Block<dyn Fn(isize, *mut AnyObject)>;
fn allowed_host(host: &str) -> bool {
    let h = host.to_ascii_lowercase();
    h == "chatgpt.com" || h.ends_with(".chatgpt.com")
}
unsafe fn string(obj: *mut AnyObject) -> Option<String> {
    if obj.is_null() {
        return None;
    }
    let p: *const c_char = msg_send![obj, UTF8String];
    (!p.is_null()).then(|| CStr::from_ptr(p).to_string_lossy().into_owned())
}
unsafe fn ca_is_anchor(ptr: *mut c_void) -> bool {
    use core_foundation::base::TCFType;
    let Some(der) = CA_DER.get() else {
        return false;
    };
    let Ok(ca) = SecCertificate::from_der(der) else {
        return false;
    };
    let mut trust = SecTrust::wrap_under_get_rule(ptr as _);
    if trust.set_anchor_certificates(&[ca]).is_err() {
        return false;
    };
    let _ = trust.set_trust_anchor_certificates_only(true);
    trust.evaluate_with_error().is_ok()
}
extern "C" fn challenge(
    _this: *mut AnyObject,
    _cmd: *const c_void,
    _wv: *mut AnyObject,
    ch: *mut AnyObject,
    handler: *mut c_void,
) {
    let (mut disposition, mut credential): (isize, *mut AnyObject) = (1, std::ptr::null_mut());
    unsafe {
        let space: *mut AnyObject = msg_send![ch, protectionSpace];
        if !space.is_null() {
            let method = string(msg_send![space, authenticationMethod]);
            let host = string(msg_send![space, host]);
            let trust: *mut c_void = msg_send![space, serverTrust];
            if method.as_deref() == Some("NSURLAuthenticationMethodServerTrust")
                && host.as_deref().is_some_and(allowed_host)
                && !trust.is_null()
                && ca_is_anchor(trust)
            {
                let cls = objc_getClass(c"NSURLCredential".as_ptr()) as *const AnyObject;
                if !cls.is_null() {
                    let cred: *mut AnyObject = msg_send![cls,credentialForTrust:trust];
                    if !cred.is_null() {
                        disposition = 0;
                        credential = cred;
                    }
                }
            }
        }
        if !handler.is_null() {
            (&*(handler as *const Completion)).call((disposition, credential));
        }
    }
}
pub fn prepare(ca_der: &[u8], wv: &WebviewWindow) {
    let _ = CA_DER.set(ca_der.to_vec());
    let _ = wv.with_webview(|w| unsafe {
        let webview = w.inner() as *mut AnyObject;
        let delegate: *mut AnyObject = msg_send![webview, navigationDelegate];
        if delegate.is_null() {
            return;
        }
        let cls = object_getClass(delegate as *const c_void);
        let sel = sel_registerName(
            c"webView:didReceiveAuthenticationChallenge:completionHandler:".as_ptr(),
        );
        let _ = class_addMethod(cls, sel, challenge as *const c_void, c"v@:@@@?".as_ptr());
    });
}
