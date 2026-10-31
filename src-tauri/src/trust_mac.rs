//! App-scoped WKWebView trust for certificates issued by this installation's proxy CA.
use objc2::msg_send;
use objc2::runtime::AnyObject;
use security_framework::{certificate::SecCertificate, trust::SecTrust};
use std::{
    ffi::{c_char, c_void, CStr},
    sync::{
        atomic::{AtomicU64, AtomicU8, Ordering},
        OnceLock,
    },
};
use tauri::Webview;

static CA_DER: OnceLock<Vec<u8>> = OnceLock::new();
static INSTALL_STATE: AtomicU8 = AtomicU8::new(0);
static CHALLENGES_REACHED: AtomicU64 = AtomicU64::new(0);
static ALLOWED_HOST_CHALLENGES: AtomicU64 = AtomicU64::new(0);
static ANCHOR_ACCEPTED_CHALLENGES: AtomicU64 = AtomicU64::new(0);
static CREDENTIALS_USED: AtomicU64 = AtomicU64::new(0);
static DELEGATE_REBOUND: AtomicU64 = AtomicU64::new(0);

const INSTALL_NOT_ATTEMPTED: u8 = 0;
const INSTALL_SCHEDULED: u8 = 1;
const INSTALL_INSTALLED: u8 = 2;
const INSTALL_ALREADY_OURS: u8 = 3;
const INSTALL_UNEXPECTED_HANDLER: u8 = 4;
const INSTALL_NO_DELEGATE: u8 = 5;
const INSTALL_VERIFY_FAILED: u8 = 6;
const INSTALL_DISPATCH_FAILED: u8 = 7;

extern "C" {
    fn object_getClass(obj: *const c_void) -> *const c_void;
    fn sel_registerName(name: *const c_char) -> *mut c_void;
    fn class_addMethod(
        cls: *const c_void,
        name: *const c_void,
        imp: *const c_void,
        types: *const c_char,
    ) -> bool;
    fn class_getInstanceMethod(cls: *const c_void, name: *const c_void) -> *mut c_void;
    fn method_getImplementation(method: *const c_void) -> *const c_void;
    fn objc_getClass(name: *const c_char) -> *const c_void;
}

type Completion = block2::Block<dyn Fn(isize, *mut AnyObject)>;

fn allowed_host(host: &str) -> bool {
    host.eq_ignore_ascii_case("chatgpt.com")
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
    }
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
    CHALLENGES_REACHED.fetch_add(1, Ordering::Relaxed);
    let (mut disposition, mut credential): (isize, *mut AnyObject) = (1, std::ptr::null_mut());
    unsafe {
        let space: *mut AnyObject = msg_send![ch, protectionSpace];
        if !space.is_null() {
            let method = string(msg_send![space, authenticationMethod]);
            let host = string(msg_send![space, host]);
            let trust: *mut c_void = msg_send![space, serverTrust];
            let host_allowed = host.as_deref().is_some_and(allowed_host);
            if host_allowed {
                ALLOWED_HOST_CHALLENGES.fetch_add(1, Ordering::Relaxed);
            }
            let anchor_accepted = method.as_deref() == Some("NSURLAuthenticationMethodServerTrust")
                && host_allowed
                && !trust.is_null()
                && ca_is_anchor(trust);
            if anchor_accepted {
                ANCHOR_ACCEPTED_CHALLENGES.fetch_add(1, Ordering::Relaxed);
                let cls = objc_getClass(c"NSURLCredential".as_ptr()) as *const AnyObject;
                if !cls.is_null() {
                    let cred: *mut AnyObject = msg_send![cls, credentialForTrust:trust];
                    if !cred.is_null() {
                        disposition = 0;
                        credential = cred;
                    }
                }
            }
        }
        if !handler.is_null() {
            if disposition == 0 && !credential.is_null() {
                CREDENTIALS_USED.fetch_add(1, Ordering::Relaxed);
            }
            (&*(handler as *const Completion)).call((disposition, credential));
        }
    }
}

fn install_state_name() -> &'static str {
    match INSTALL_STATE.load(Ordering::Acquire) {
        INSTALL_SCHEDULED => "scheduled",
        INSTALL_INSTALLED => "installed",
        INSTALL_ALREADY_OURS => "already_ours",
        INSTALL_UNEXPECTED_HANDLER => "unexpected_handler",
        INSTALL_NO_DELEGATE => "no_delegate",
        INSTALL_VERIFY_FAILED => "verify_failed",
        INSTALL_DISPATCH_FAILED => "dispatch_failed",
        _ => "not_attempted",
    }
}

/// Safe aggregate diagnostics only: no host strings, certificates, or credentials.
pub fn diagnostics() -> serde_json::Value {
    serde_json::json!({
        "install_state": install_state_name(),
        "challenge_reached": CHALLENGES_REACHED.load(Ordering::Relaxed),
        "allowed_host_challenges": ALLOWED_HOST_CHALLENGES.load(Ordering::Relaxed),
        "anchor_accepted_challenges": ANCHOR_ACCEPTED_CHALLENGES.load(Ordering::Relaxed),
        "credentials_used": CREDENTIALS_USED.load(Ordering::Relaxed),
        "delegate_rebound": DELEGATE_REBOUND.load(Ordering::Relaxed) > 0,
    })
}

unsafe fn install_handler(webview: *mut AnyObject) {
    let delegate: *mut AnyObject = msg_send![webview, navigationDelegate];
    if delegate.is_null() {
        INSTALL_STATE.store(INSTALL_NO_DELEGATE, Ordering::Release);
        return;
    }
    let cls = object_getClass(delegate as *const c_void);
    if cls.is_null() {
        INSTALL_STATE.store(INSTALL_VERIFY_FAILED, Ordering::Release);
        return;
    }
    let selector =
        sel_registerName(c"webView:didReceiveAuthenticationChallenge:completionHandler:".as_ptr());
    let method = class_getInstanceMethod(cls, selector);
    let already_ours = if method.is_null() {
        let _added = class_addMethod(
            cls,
            selector,
            challenge as *const c_void,
            c"v@:@@@?".as_ptr(),
        );
        false
    } else if method_getImplementation(method) == challenge as *const c_void {
        true
    } else {
        // Preserve any implementation installed by WebKit/Wry/another delegate.
        INSTALL_STATE.store(INSTALL_UNEXPECTED_HANDLER, Ordering::Release);
        return;
    };
    let verify_method = class_getInstanceMethod(cls, selector);
    let implementation_matches = !verify_method.is_null()
        && method_getImplementation(verify_method) == challenge as *const c_void;
    let responds: bool = msg_send![delegate, respondsToSelector: selector];
    if implementation_matches && responds {
        // WebKit snapshots optional delegate capabilities at assignment time.
        // Wry assigned this delegate before the challenge IMP was added, so
        // rebind the same strongly retained object to refresh those flags.
        let Some(retained_delegate) = objc2::rc::Retained::<AnyObject>::retain(delegate) else {
            INSTALL_STATE.store(INSTALL_VERIFY_FAILED, Ordering::Release);
            return;
        };
        let nil: *mut AnyObject = std::ptr::null_mut();
        let _: () = msg_send![webview, setNavigationDelegate: nil];
        let _: () = msg_send![webview, setNavigationDelegate: &*retained_delegate];
        let responds_after_rebind: bool = msg_send![delegate, respondsToSelector: selector];
        let verify_after_rebind = class_getInstanceMethod(cls, selector);
        let implementation_matches_after_rebind = !verify_after_rebind.is_null()
            && method_getImplementation(verify_after_rebind) == challenge as *const c_void;
        if responds_after_rebind && implementation_matches_after_rebind {
            DELEGATE_REBOUND.fetch_add(1, Ordering::Relaxed);
            INSTALL_STATE.store(
                if already_ours {
                    INSTALL_ALREADY_OURS
                } else {
                    INSTALL_INSTALLED
                },
                Ordering::Release,
            );
        } else {
            INSTALL_STATE.store(INSTALL_VERIFY_FAILED, Ordering::Release);
        }
    } else {
        INSTALL_STATE.store(INSTALL_VERIFY_FAILED, Ordering::Release);
    }
}

/// Schedules delegate installation on the webview main thread. The closure is
/// asynchronous during app setup; diagnostics report its verified final state.
pub fn prepare(ca_der: &[u8], webview: &Webview<tauri::Wry>) -> Result<(), String> {
    if let Some(existing) = CA_DER.get() {
        if existing.as_slice() != ca_der {
            return Err("trust_ca_configuration_conflict".into());
        }
    } else {
        CA_DER
            .set(ca_der.to_vec())
            .map_err(|_| "trust_ca_configuration_conflict".to_string())?;
    }
    INSTALL_STATE.store(INSTALL_SCHEDULED, Ordering::Release);
    let result = webview.with_webview(move |platform| unsafe {
        install_handler(platform.inner() as *mut AnyObject);
    });
    if result.is_err() {
        INSTALL_STATE.store(INSTALL_DISPATCH_FAILED, Ordering::Release);
        return Err("trust_delegate_dispatch_failed".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::allowed_host;

    #[test]
    fn trust_override_is_limited_to_proxy_intercepted_chatgpt_host() {
        assert!(allowed_host("chatgpt.com"));
        assert!(allowed_host("CHATGPT.COM"));
        assert!(!allowed_host("api.chatgpt.com"));
        assert!(!allowed_host("chatgpt.com.evil.test"));
        assert!(!allowed_host("example.com"));
    }
}
