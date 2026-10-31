//! Request-local attribution from observed provider headers, not authentication.
//! Credential material is used transiently and never returned or retained.
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use hyper::HeaderMap;
use ring::{
    digest, hmac,
    rand::{SecureRandom, SystemRandom},
};
use std::sync::atomic::{AtomicU64, Ordering};

pub(crate) struct RequestIdentity {
    key: hmac::Key,
    next: AtomicU64,
}

impl RequestIdentity {
    pub(crate) fn new() -> std::io::Result<Self> {
        let mut secret = [0; 32];
        SystemRandom::new()
            .fill(&mut secret)
            .map_err(|_| std::io::Error::other("identity initialization failed"))?;
        Ok(Self {
            key: hmac::Key::new(hmac::HMAC_SHA256, &secret),
            next: AtomicU64::new(1),
        })
    }

    pub(crate) fn resolve(&self, configured: &str, headers: &HeaderMap) -> Result<String, ()> {
        if !crate::config::valid_capture_profile_scope(configured) {
            return Err(());
        }
        // Explicit caller-attributed scopes retain the existing injection contract.
        if !configured.starts_with("provisional:") {
            return Ok(configured.to_owned());
        }
        let authorization = one(headers, "authorization")?;
        let account_header = one(headers, "chatgpt-account-id")?;
        if account_header.is_some_and(|id| !valid_id(id)) {
            return Err(());
        }
        let bearer = authorization
            .and_then(|value| value.split_once(' '))
            .filter(|(scheme, token)| scheme.eq_ignore_ascii_case("bearer") && !token.is_empty())
            .map(|(_, token)| token);
        if let Some(token) = bearer {
            if let Some(payload) = jwt_payload(token) {
                if let Some(auth) = payload.get("https://api.openai.com/auth") {
                    if !auth.is_object() {
                        return Err(());
                    }
                    let account = claim(auth, "chatgpt_account_id")?;
                    let user = claim(auth, "chatgpt_user_id")?.or(claim(auth, "user_id")?);
                    if let (Some(header), Some(account)) = (account_header, account) {
                        if header != account {
                            return Err(());
                        }
                    }
                    if let (Some(account), Some(user)) = (account, user) {
                        let data =
                            serde_json::to_vec(&("chatgpt-request-identity-v1", account, user))
                                .map_err(|_| ())?;
                        return Ok(format!(
                            "chatgpt:observed:{}",
                            hex(digest::digest(&digest::SHA256, &data).as_ref())
                        ));
                    }
                }
            }
        }
        // Unknown ownership must not share a global default. An opaque credential
        // is isolated by a process-keyed HMAC; anonymous requests are unique.
        // Rotations/restarts may split provisional histories, never merge them.
        let nonce = if authorization.is_none() {
            self.next.fetch_add(1, Ordering::Relaxed)
        } else {
            0
        };
        let data = serde_json::to_vec(&(
            "chatgpt-provisional-v1",
            configured,
            authorization,
            account_header,
            nonce,
        ))
        .map_err(|_| ())?;
        Ok(format!(
            "provisional:request:{}",
            hex(hmac::sign(&self.key, &data).as_ref())
        ))
    }
}

fn one<'a>(headers: &'a HeaderMap, name: &str) -> Result<Option<&'a str>, ()> {
    let mut values = headers.get_all(name).iter();
    let value = values
        .next()
        .map(|value| value.to_str().map_err(|_| ()))
        .transpose()?;
    if values.next().is_some() || value.is_some_and(|value| value.len() > 16384) {
        return Err(());
    }
    Ok(value)
}
fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 256
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_:".contains(&b))
}
fn claim<'a>(value: &'a serde_json::Value, name: &str) -> Result<Option<&'a str>, ()> {
    match value.get(name) {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(serde_json::Value::String(id)) if valid_id(id) => Ok(Some(id)),
        _ => Err(()),
    }
}
fn jwt_payload(token: &str) -> Option<serde_json::Value> {
    let parts: Vec<_> = token.split('.').collect();
    if parts.len() != 3 || parts[1].len() > 12000 {
        return None;
    }
    let bytes = URL_SAFE_NO_PAD.decode(parts[1]).ok()?;
    serde_json::from_slice(&bytes).ok()
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    fn headers(account: &str, user: &str, signature: &str) -> HeaderMap {
        let payload = serde_json::json!({"https://api.openai.com/auth":{"chatgpt_account_id":account,"chatgpt_user_id":user}});
        let token = format!(
            "Bearer e30.{}.{}",
            URL_SAFE_NO_PAD.encode(payload.to_string()),
            signature
        );
        let mut h = HeaderMap::new();
        h.insert("authorization", token.parse().unwrap());
        h
    }
    #[test]
    fn rotations_are_stable_and_users_accounts_and_delayed_requests_are_isolated() {
        let resolver = RequestIdentity::new().unwrap();
        let first = resolver
            .resolve(
                "provisional:default",
                &headers("account-a", "user-a", "first"),
            )
            .unwrap();
        let second = resolver
            .resolve(
                "provisional:default",
                &headers("account-b", "user-a", "second"),
            )
            .unwrap();
        assert_ne!(first, second);
        assert_ne!(
            first,
            resolver
                .resolve(
                    "provisional:default",
                    &headers("account-a", "user-b", "third")
                )
                .unwrap()
        );
        assert_eq!(
            first,
            resolver
                .resolve(
                    "provisional:default",
                    &headers("account-a", "user-a", "rotated")
                )
                .unwrap()
        );
        assert!(first.starts_with("chatgpt:observed:"));
        assert!(!first.contains("account-a"));
    }
    #[test]
    fn ambiguous_headers_and_account_conflicts_are_rejected() {
        let resolver = RequestIdentity::new().unwrap();
        let mut h = headers("account-a", "user-a", "signature");
        h.insert("chatgpt-account-id", "account-b".parse().unwrap());
        assert!(resolver.resolve("provisional:default", &h).is_err());
        h.insert("chatgpt-account-id", "account-a".parse().unwrap());
        assert!(resolver.resolve("provisional:default", &h).is_ok());
        h.append("authorization", "Bearer another".parse().unwrap());
        assert!(resolver.resolve("provisional:default", &h).is_err());
    }
    #[test]
    fn unknown_credentials_and_anonymous_requests_never_share_global_scope() {
        let resolver = RequestIdentity::new().unwrap();
        let mut h = HeaderMap::new();
        h.insert(
            "authorization",
            "Bearer PRIVATE_TOKEN_SENTINEL".parse().unwrap(),
        );
        let first = resolver.resolve("provisional:default", &h).unwrap();
        assert!(first.starts_with("provisional:request:"));
        assert!(!first.contains("PRIVATE_TOKEN"));
        assert_eq!(first, resolver.resolve("provisional:default", &h).unwrap());
        assert_ne!(
            first,
            RequestIdentity::new()
                .unwrap()
                .resolve("provisional:default", &h)
                .unwrap()
        );
        assert_ne!(
            resolver
                .resolve("provisional:default", &HeaderMap::new())
                .unwrap(),
            resolver
                .resolve("provisional:default", &HeaderMap::new())
                .unwrap()
        );
        assert_eq!(
            resolver.resolve("profile:caller-attributed", &h).unwrap(),
            "profile:caller-attributed"
        );
    }
}
