//! On-demand provider WebView embedded in the native main window.
use ring::rand::SecureRandom;
use serde::Serialize;
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tauri::{Manager, Webview, WebviewBuilder, WebviewWindow, Wry};

/// Build only the provider's own URL. Conversation IDs are canonical UUIDs;
/// callers cannot supply a URL, path, or other navigation target.
pub fn target_url(conversation_id: Option<&str>) -> Result<tauri::Url, String> {
    let path = match conversation_id {
        None => "/".to_string(),
        Some(id) if valid_conversation_id(id) => format!("/c/{id}"),
        Some(_) => return Err("invalid_conversation_id".into()),
    };
    tauri::Url::parse(&format!("https://chatgpt.com{path}"))
        .map_err(|_| "invalid_conversation_id".into())
}

fn valid_conversation_id(id: &str) -> bool {
    let parts: Vec<_> = id.split('-').collect();
    parts.len() == 5
        && [8, 4, 4, 4, 12]
            .iter()
            .zip(parts)
            .all(|(len, part)| part.len() == *len && part.bytes().all(|b| b.is_ascii_hexdigit()))
}

fn allowed_provider_navigation(url: &tauri::Url) -> bool {
    // Federated login may visit other HTTPS origins. Privileged IPC remains
    // restricted to the native main webview regardless of the provider URL.
    url.scheme() == "https" || url.as_str() == "about:blank"
}

#[derive(Clone, Serialize)]
pub struct ProviderViewStatus {
    pub state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<String>,
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ChatGptConnectionState {
    NotOpen,
    Loading,
    Connected,
    SignedOut,
    Unavailable,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct ChatGptConnectionStatus {
    pub state: ChatGptConnectionState,
    pub message: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account_scope: Option<String>,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct ProjectAssignmentOutcome {
    pub state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_after: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_title: Option<String>,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct ProjectRenameOutcome {
    pub state: String,
    pub verified: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_after: Option<u64>,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct ConversationActionOutcome {
    pub state: String,
    pub verified: bool,
    pub conversation_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub archived: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pinned: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_after: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChatgptPinSnapshotOutcome {
    pub complete: bool,
    pub account_scope: String,
    pub pinned_ids: Vec<String>,
    pub error_code: Option<String>,
    pub retry_after: Option<u64>,
}

impl ChatGptConnectionStatus {
    fn new(state: ChatGptConnectionState) -> Self {
        let message = match state {
            ChatGptConnectionState::NotOpen => "Open ChatGPT to check your connection.",
            ChatGptConnectionState::Loading => "ChatGPT is still loading.",
            ChatGptConnectionState::Connected => "Connected to ChatGPT.",
            ChatGptConnectionState::SignedOut => "Sign in to ChatGPT to continue.",
            ChatGptConnectionState::Unavailable => {
                "ChatGPT connection could not be checked. Try again."
            }
        };
        Self {
            state,
            message,
            account_scope: None,
        }
    }

    fn connected(account_scope: Option<String>) -> Self {
        let mut status = Self::new(ChatGptConnectionState::Connected);
        status.account_scope = account_scope;
        status
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum ProbePoll {
    Pending,
    Complete(ChatGptConnectionState, Option<String>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum AssignmentPoll {
    Pending,
    Complete(ProjectAssignmentOutcome),
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum RenamePoll {
    Pending,
    Complete(ProjectRenameOutcome),
}

enum ActionPoll {
    Pending,
    Complete(ConversationActionOutcome),
}

#[derive(Debug, PartialEq, Eq)]
enum PinSnapshotPoll {
    Pending,
    Complete(Vec<String>),
    Failed {
        error_code: String,
        retry_after: Option<u64>,
    },
}

fn parse_pin_snapshot_callback(
    raw: &str,
    expected_nonce: &str,
    expected_scope: &str,
) -> Option<PinSnapshotPoll> {
    const MAX_CALLBACK_BYTES: usize = 64 * 1024;
    const MAX_PINNED_IDS: usize = 1_000;
    if raw.len() > MAX_CALLBACK_BYTES {
        return None;
    }
    let mut value: serde_json::Value = serde_json::from_str(raw).ok()?;
    for _ in 0..3 {
        let Some(nested) = value.as_str() else { break };
        if nested.len() > MAX_CALLBACK_BYTES {
            return None;
        }
        value = serde_json::from_str(nested).ok()?;
    }
    let object = value.as_object()?;
    if object.get("nonce")?.as_str()? != expected_nonce
        || object.get("cancelled")?.as_bool()? != false
    {
        return None;
    }
    match object.get("state")?.as_str()? {
        "running" if object.len() == 3 => Some(PinSnapshotPoll::Pending),
        "complete" if object.len() == 6 => {
            if object.keys().any(|key| {
                !matches!(
                    key.as_str(),
                    "nonce" | "state" | "cancelled" | "complete" | "account_scope" | "pinned_ids"
                )
            }) || object.get("complete")?.as_bool()? != true
                || object.get("account_scope")?.as_str()? != expected_scope
            {
                return None;
            }
            let ids = object.get("pinned_ids")?.as_array()?;
            if ids.len() > MAX_PINNED_IDS {
                return None;
            }
            let mut seen = HashSet::with_capacity(ids.len());
            let mut pinned_ids = Vec::with_capacity(ids.len());
            for value in ids {
                let id = value.as_str()?;
                if !valid_conversation_id(id)
                    || id.bytes().any(|byte| byte.is_ascii_uppercase())
                    || !seen.insert(id)
                {
                    return None;
                }
                pinned_ids.push(id.to_owned());
            }
            Some(PinSnapshotPoll::Complete(pinned_ids))
        }
        "failed" if (4..=5).contains(&object.len()) => {
            if object.keys().any(|key| {
                !matches!(
                    key.as_str(),
                    "nonce" | "state" | "cancelled" | "error_code" | "retry_after"
                )
            }) {
                return None;
            }
            let error_code = object.get("error_code")?.as_str()?;
            if !matches!(
                error_code,
                "invalid_request"
                    | "signed_out"
                    | "account_unavailable"
                    | "account_changed"
                    | "rate_limited"
                    | "request_failed"
                    | "invalid_pin_list"
                    | "incomplete_pin_list"
                    | "pin_list_too_large"
                    | "cancelled"
            ) {
                return None;
            }
            let retry_after = if error_code == "rate_limited" {
                let retry = object.get("retry_after")?.as_u64()?;
                if !(1..=31_536_001).contains(&retry) {
                    return None;
                }
                Some(retry)
            } else if object.contains_key("retry_after") {
                return None;
            } else {
                None
            };
            Some(PinSnapshotPoll::Failed {
                error_code: error_code.into(),
                retry_after,
            })
        }
        _ => None,
    }
}

fn parse_conversation_action_callback(
    raw: &str,
    expected_nonce: &str,
    conversation_id: &str,
    action: &str,
    desired: bool,
) -> Option<ActionPoll> {
    if raw.len() > 8 * 1024 {
        return None;
    }
    let mut value: serde_json::Value = serde_json::from_str(raw).ok()?;
    for _ in 0..3 {
        let Some(nested) = value.as_str() else { break };
        if nested.len() > 8 * 1024 {
            return None;
        }
        value = serde_json::from_str(nested).ok()?;
    }
    let object = value.as_object()?;
    if object.get("nonce")?.as_str()? != expected_nonce {
        return None;
    }
    match object.get("state")?.as_str()? {
        "running" if object.len() == 3 && object.get("cancelled")?.as_bool()? == false => {
            Some(ActionPoll::Pending)
        }
        "applied" => {
            let state_key = if action == "archive" {
                "archived"
            } else {
                "pinned"
            };
            if object.len() != 6
                || object.get("cancelled")?.as_bool()? != false
                || object.get("verified")?.as_bool()? != true
                || object.get("conversation_id")?.as_str()? != conversation_id
                || object.get(state_key)?.as_bool()? != desired
                || object.keys().any(|key| {
                    !matches!(
                        key.as_str(),
                        "nonce"
                            | "state"
                            | "cancelled"
                            | "verified"
                            | "conversation_id"
                            | "archived"
                            | "pinned"
                    )
                })
                || object.contains_key(if action == "archive" {
                    "pinned"
                } else {
                    "archived"
                })
            {
                return None;
            }
            Some(ActionPoll::Complete(ConversationActionOutcome {
                state: "applied".into(),
                verified: true,
                conversation_id: conversation_id.to_owned(),
                archived: (action == "archive").then_some(desired),
                pinned: (action == "pin").then_some(desired),
                error_code: None,
                retry_after: None,
            }))
        }
        "failed" if (4..=5).contains(&object.len()) => {
            if object.get("cancelled")?.as_bool()? != false
                || object.keys().any(|key| {
                    !matches!(
                        key.as_str(),
                        "nonce" | "state" | "cancelled" | "error_code" | "retry_after"
                    )
                })
            {
                return None;
            }
            let error_code = object.get("error_code")?.as_str()?;
            if !matches!(
                error_code,
                "cancelled"
                    | "invalid_request"
                    | "signed_out"
                    | "account_unavailable"
                    | "account_changed"
                    | "conversation_unavailable"
                    | "request_failed"
                    | "rate_limited"
                    | "verification_failed"
            ) {
                return None;
            }
            let retry_after = match object.get("retry_after") {
                None => None,
                Some(value) => Some(value.as_u64()?),
            };
            if retry_after.is_some_and(|value| !(1..=31_536_001).contains(&value)) {
                return None;
            }
            Some(ActionPoll::Complete(ConversationActionOutcome {
                state: "failed".into(),
                verified: false,
                conversation_id: conversation_id.to_owned(),
                archived: None,
                pinned: None,
                error_code: Some(error_code.into()),
                retry_after,
            }))
        }
        _ => None,
    }
}

fn parse_project_rename_callback(raw: &str, expected_nonce: &str) -> Option<RenamePoll> {
    if raw.len() > 8 * 1024 {
        return None;
    }
    let mut value: serde_json::Value = serde_json::from_str(raw).ok()?;
    for _ in 0..3 {
        let Some(nested) = value.as_str() else { break };
        if nested.len() > 8 * 1024 {
            return None;
        }
        value = serde_json::from_str(nested).ok()?;
    }
    let object = value.as_object()?;
    if object.get("nonce")?.as_str()? != expected_nonce {
        return None;
    }
    match object.get("state")?.as_str()? {
        "running" if object.len() == 3 && object.get("cancelled")?.as_bool()? == false => {
            Some(RenamePoll::Pending)
        }
        "applied"
            if object.len() == 4
                && object.get("cancelled")?.as_bool()? == false
                && object.get("verified")?.as_bool()? == true =>
        {
            if object
                .keys()
                .any(|key| !matches!(key.as_str(), "nonce" | "state" | "cancelled" | "verified"))
            {
                return None;
            }
            Some(RenamePoll::Complete(ProjectRenameOutcome {
                state: "applied".into(),
                verified: true,
                error_code: None,
                retry_after: None,
            }))
        }
        "failed" if (4..=5).contains(&object.len()) => {
            if object.keys().any(|key| {
                !matches!(
                    key.as_str(),
                    "nonce" | "state" | "cancelled" | "error_code" | "retry_after"
                )
            }) || object.get("cancelled")?.as_bool()? != false
            {
                return None;
            }
            let error_code = object.get("error_code")?.as_str()?;
            if !matches!(
                error_code,
                "cancelled"
                    | "invalid_request"
                    | "signed_out"
                    | "account_unavailable"
                    | "account_changed"
                    | "project_shape_unavailable"
                    | "request_failed"
                    | "rate_limited"
                    | "verification_failed"
            ) {
                return None;
            }
            let retry_after = match object.get("retry_after") {
                None => None,
                Some(value) if error_code == "rate_limited" => Some(value.as_u64()?.clamp(1, 1800)),
                _ => return None,
            };
            Some(RenamePoll::Complete(ProjectRenameOutcome {
                state: "failed".into(),
                verified: false,
                error_code: Some(error_code.into()),
                retry_after,
            }))
        }
        _ => None,
    }
}

fn parse_assignment_callback(raw: &str, expected_nonce: &str) -> Option<AssignmentPoll> {
    if raw.len() > 16 * 1024 {
        return None;
    }
    let mut value: serde_json::Value = serde_json::from_str(raw).ok()?;
    for _ in 0..3 {
        let Some(nested) = value.as_str() else { break };
        if nested.len() > 16 * 1024 {
            return None;
        }
        value = serde_json::from_str(nested).ok()?;
    }
    let object = value.as_object()?;
    if object.get("nonce")?.as_str()? != expected_nonce {
        return None;
    }
    match object.get("state")?.as_str()? {
        "running" if object.len() == 3 && object.get("cancelled")?.as_bool()? == false => {
            Some(AssignmentPoll::Pending)
        }
        "applied" if (2..=4).contains(&object.len()) => {
            if object.keys().any(|key| {
                !matches!(
                    key.as_str(),
                    "nonce" | "state" | "project_title" | "cancelled"
                )
            }) {
                return None;
            }
            let project_title = match object.get("project_title") {
                None | Some(serde_json::Value::Null) => None,
                Some(serde_json::Value::String(title))
                    if !title.trim().is_empty() && title.len() <= 4096 =>
                {
                    Some(title.clone())
                }
                _ => return None,
            };
            Some(AssignmentPoll::Complete(ProjectAssignmentOutcome {
                state: "applied".into(),
                error_code: None,
                retry_after: None,
                project_title,
            }))
        }
        "failed" if (3..=5).contains(&object.len()) => {
            if object.keys().any(|key| {
                !matches!(
                    key.as_str(),
                    "nonce" | "state" | "error_code" | "retry_after" | "cancelled"
                )
            }) {
                return None;
            }
            let code = object.get("error_code")?.as_str()?;
            if !matches!(
                code,
                "invalid_context"
                    | "signed_out"
                    | "account_unavailable"
                    | "account_changed"
                    | "conversation_unavailable"
                    | "project_catalog_unavailable"
                    | "project_unavailable"
                    | "project_name_unavailable"
                    | "verification_failed"
                    | "rate_limited"
                    | "request_failed"
            ) {
                return None;
            }
            let retry_after = match object.get("retry_after") {
                None => None,
                Some(value) if code == "rate_limited" => Some(value.as_u64()?.clamp(1, 1800)),
                _ => return None,
            };
            Some(AssignmentPoll::Complete(ProjectAssignmentOutcome {
                state: "failed".into(),
                error_code: Some(code.into()),
                retry_after,
                project_title: None,
            }))
        }
        "cancelled" if object.len() == 3 && object.get("cancelled")?.as_bool()? => {
            Some(AssignmentPoll::Complete(ProjectAssignmentOutcome {
                state: "cancelled".into(),
                error_code: None,
                retry_after: None,
                project_title: None,
            }))
        }
        _ => None,
    }
}

fn valid_observed_scope(scope: &str) -> bool {
    scope
        .strip_prefix("chatgpt:observed:")
        .is_some_and(|digest| {
            digest.len() == 64
                && digest
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        })
}

fn exact_chatgpt_origin(url: &tauri::Url) -> bool {
    url.scheme() == "https"
        && url.host_str() == Some("chatgpt.com")
        && url.port().is_none()
        && url.username().is_empty()
        && url.password().is_none()
}

fn current_chatgpt_conversation_id(url: &tauri::Url) -> Option<String> {
    if !exact_chatgpt_origin(url) || url.query().is_some() || url.fragment().is_some() {
        return None;
    }
    let id = url.path().strip_prefix("/c/")?;
    if !valid_conversation_id(id) || id.bytes().any(|byte| byte.is_ascii_uppercase()) {
        return None;
    }
    Some(id.to_owned())
}

#[derive(Clone, Debug, Default)]
struct PageMetadata {
    exact_origin: bool,
    conversation_id: Option<String>,
    account_scope: Option<String>,
    navigation_generation: u64,
    next_sequence: u64,
    applied_sequence: u64,
}

fn parse_page_metadata_callback(raw: &str, expected_nonce: &str) -> Option<(bool, Option<String>)> {
    let mut value: serde_json::Value = serde_json::from_str(raw).ok()?;
    for _ in 0..3 {
        let Some(nested) = value.as_str() else { break };
        value = serde_json::from_str(nested).ok()?;
    }
    let object = value.as_object()?;
    if object.len() != 3 || object.get("nonce")?.as_str()? != expected_nonce {
        return None;
    }
    let exact_origin = object.get("exact_origin")?.as_bool()?;
    let conversation_id = match object.get("conversation_id")? {
        serde_json::Value::Null => None,
        serde_json::Value::String(id)
            if exact_origin
                && valid_conversation_id(id)
                && id.bytes().all(|byte| !byte.is_ascii_uppercase()) =>
        {
            Some(id.clone())
        }
        _ => return None,
    };
    Some((exact_origin, conversation_id))
}

fn page_metadata_script(nonce: &str) -> Option<String> {
    let nonce_json = serde_json::to_string(nonce).ok()?;
    Some(format!(
        "({})({nonce_json})",
        include_str!("provider_page_metadata.js")
    ))
}

fn request_page_metadata(webview: &Webview<Wry>, page: &Arc<Mutex<PageMetadata>>) {
    let Some(nonce) = fresh_probe_nonce() else {
        return;
    };
    let (generation, sequence) = match page.lock() {
        Ok(mut metadata) => {
            metadata.next_sequence = metadata.next_sequence.saturating_add(1);
            (metadata.navigation_generation, metadata.next_sequence)
        }
        Err(_) => return,
    };
    let Some(script) = page_metadata_script(&nonce) else {
        return;
    };
    let page = page.clone();
    let _ = webview.eval_with_callback(script, move |value| {
        let Some((exact_origin, conversation_id)) = parse_page_metadata_callback(&value, &nonce)
        else {
            return;
        };
        if let Ok(mut metadata) = page.lock() {
            if metadata.navigation_generation == generation && sequence >= metadata.applied_sequence
            {
                metadata.exact_origin = exact_origin;
                metadata.conversation_id = conversation_id;
                if !exact_origin {
                    metadata.account_scope = None;
                }
                metadata.applied_sequence = sequence;
            }
        }
    });
}

fn parse_probe_callback(raw: &str, expected_nonce: &str) -> Option<ProbePoll> {
    let mut value: serde_json::Value = serde_json::from_str(raw).ok()?;
    for _ in 0..3 {
        let Some(nested) = value.as_str() else {
            break;
        };
        value = serde_json::from_str(nested).ok()?;
    }
    let object = value.as_object()?;
    if !(2..=3).contains(&object.len()) {
        return None;
    }
    if object.get("nonce")?.as_str()? != expected_nonce {
        return None;
    }
    if object
        .keys()
        .any(|key| !matches!(key.as_str(), "nonce" | "state" | "account_scope"))
    {
        return None;
    }
    match object.get("state")?.as_str()? {
        "checking" if object.len() == 2 => Some(ProbePoll::Pending),
        "connected" => {
            if object.len() == 3 && !object.contains_key("account_scope") {
                return None;
            }
            let scope = match object.get("account_scope") {
                None | Some(serde_json::Value::Null) => None,
                Some(serde_json::Value::String(scope)) if valid_observed_scope(scope) => {
                    Some(scope.clone())
                }
                _ => return None,
            };
            Some(ProbePoll::Complete(
                ChatGptConnectionState::Connected,
                scope,
            ))
        }
        "signed_out" if object.len() == 2 => {
            Some(ProbePoll::Complete(ChatGptConnectionState::SignedOut, None))
        }
        "unavailable" if object.len() == 2 => Some(ProbePoll::Complete(
            ChatGptConnectionState::Unavailable,
            None,
        )),
        _ => None,
    }
}

fn fresh_probe_nonce() -> Option<String> {
    let mut bytes = [0u8; 16];
    ring::rand::SystemRandom::new().fill(&mut bytes).ok()?;
    Some(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn cleanup_probe(webview: &Webview<Wry>, key_json: &str) {
    let _ = webview.eval(&format!("delete window[{key_json}]"));
}

fn should_prepare_session(navigated: bool) -> bool {
    !navigated
}

fn pin_cooldown_remaining(
    deadlines: &mut HashMap<String, Instant>,
    expected_scope: &str,
    now: Instant,
) -> Option<Duration> {
    deadlines.retain(|_, deadline| *deadline > now);
    deadlines
        .get(expected_scope)
        .map(|deadline| deadline.saturating_duration_since(now))
}

fn extend_pin_cooldown(
    deadlines: &mut HashMap<String, Instant>,
    expected_scope: &str,
    deadline: Instant,
) {
    deadlines
        .entry(expected_scope.to_owned())
        .and_modify(|existing| *existing = (*existing).max(deadline))
        .or_insert(deadline);
}

struct Inner {
    webview: Webview<Wry>,
    navigated: bool,
    loaded: bool,
    visible: bool,
}

pub struct ProviderViewState {
    inner: Mutex<Inner>,
    page: Arc<Mutex<PageMetadata>>,
    pin_snapshot_cooldowns: Mutex<HashMap<String, Instant>>,
    proxy_enabled: bool,
    direct_smoke_enabled: bool,
}

impl ProviderViewState {
    /// Assign or unassign one conversation through the already-open ChatGPT
    /// session. Only a server-confirmed result and safe title leave the page.
    pub async fn set_conversation_project(
        &self,
        conversation_id: &str,
        project_id: Option<&str>,
        expected_scope: &str,
    ) -> Result<ProjectAssignmentOutcome, String> {
        if !valid_conversation_id(conversation_id)
            || !valid_observed_scope(expected_scope)
            || project_id.is_some_and(|id| {
                !id.starts_with("g-p-")
                    || id.len() > 132
                    || !id
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
            })
        {
            return Err("invalid_assignment".into());
        }
        let (webview, loaded) = self
            .inner
            .lock()
            .map_err(|_| "provider_unavailable".to_string())
            .map(|inner| (inner.webview.clone(), inner.loaded))?;
        let generation = {
            let page = self
                .page
                .lock()
                .map_err(|_| "provider_unavailable".to_string())?;
            if !page.exact_origin {
                return Err("chatgpt_not_open".into());
            }
            if page.account_scope.as_deref() != Some(expected_scope) {
                return Err("chatgpt_account_changed".into());
            }
            page.navigation_generation
        };
        if !loaded {
            return Err("chatgpt_loading".into());
        }
        let nonce = fresh_probe_nonce().ok_or_else(|| "provider_unavailable".to_string())?;
        let key = format!("__chatHubProjectAssignment_{nonce}");
        let key_json = serde_json::to_string(&key).map_err(|_| "provider_unavailable")?;
        let nonce_json = serde_json::to_string(&nonce).map_err(|_| "provider_unavailable")?;
        let scope_json = serde_json::to_string(expected_scope).map_err(|_| "invalid_assignment")?;
        let conversation_json =
            serde_json::to_string(conversation_id).map_err(|_| "invalid_assignment")?;
        let project_json = serde_json::to_string(&project_id).map_err(|_| "invalid_assignment")?;
        let script = format!(
            "({})( {key_json}, {nonce_json}, {scope_json}, {conversation_json}, {project_json} )",
            include_str!("project_assignment.js")
        );
        webview
            .eval(&script)
            .map_err(|_| "provider_unavailable".to_string())?;
        let deadline = Instant::now() + Duration::from_secs(120);
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                cleanup_probe(&webview, &key_json);
                return Err("project_assignment_timeout".into());
            }
            let (tx, rx) = tokio::sync::oneshot::channel();
            let tx = Mutex::new(Some(tx));
            if webview
                .eval_with_callback(
                    format!("JSON.stringify(window[{key_json}]||null)"),
                    move |value| {
                        if let Ok(mut slot) = tx.lock() {
                            if let Some(tx) = slot.take() {
                                let _ = tx.send(value);
                            }
                        }
                    },
                )
                .is_err()
            {
                cleanup_probe(&webview, &key_json);
                return Err("provider_unavailable".into());
            }
            match tokio::time::timeout(remaining.min(Duration::from_secs(2)), rx).await {
                Ok(Ok(raw)) => match parse_assignment_callback(&raw, &nonce) {
                    Some(AssignmentPoll::Pending) => {}
                    Some(AssignmentPoll::Complete(outcome)) => {
                        let same_page = self.page.lock().ok().is_some_and(|page| {
                            page.navigation_generation == generation
                                && page.exact_origin
                                && page.account_scope.as_deref() == Some(expected_scope)
                        });
                        cleanup_probe(&webview, &key_json);
                        return if same_page {
                            Ok(outcome)
                        } else {
                            Err("chatgpt_page_changed".into())
                        };
                    }
                    None => {
                        cleanup_probe(&webview, &key_json);
                        return Err("provider_unavailable".into());
                    }
                },
                Ok(Err(_)) => {
                    cleanup_probe(&webview, &key_json);
                    return Err("provider_unavailable".into());
                }
                Err(_) => {}
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    /// Read a complete, account-bound pin listing without changing provider
    /// state. The returned identifiers are kept inside the native process.
    pub async fn read_chatgpt_pin_snapshot(
        &self,
        expected_scope: &str,
    ) -> Result<ChatgptPinSnapshotOutcome, String> {
        if !valid_observed_scope(expected_scope) {
            return Err("invalid_request".into());
        }
        if self
            .pin_snapshot_cooldown_remaining(expected_scope)?
            .is_some()
        {
            return Err("chatgpt_pin_rate_limited".into());
        }
        let (webview, loaded) = self
            .inner
            .lock()
            .map_err(|_| "provider_unavailable".to_string())
            .map(|inner| (inner.webview.clone(), inner.loaded))?;
        let generation = {
            let page = self
                .page
                .lock()
                .map_err(|_| "provider_unavailable".to_string())?;
            if !page.exact_origin {
                return Err("chatgpt_not_open".into());
            }
            if page.account_scope.as_deref() != Some(expected_scope) {
                return Err("chatgpt_account_changed".into());
            }
            page.navigation_generation
        };
        if !loaded {
            return Err("chatgpt_loading".into());
        }
        let nonce = fresh_probe_nonce().ok_or_else(|| "provider_unavailable".to_string())?;
        let key = format!("__chatHubPinSnapshot_{nonce}");
        let key_json = serde_json::to_string(&key).map_err(|_| "provider_unavailable")?;
        let nonce_json = serde_json::to_string(&nonce).map_err(|_| "provider_unavailable")?;
        let scope_json = serde_json::to_string(expected_scope).map_err(|_| "invalid_request")?;
        let script = format!(
            "({})( {key_json}, {nonce_json}, {scope_json} )",
            include_str!("pin_snapshot.js")
        );
        webview
            .eval(&script)
            .map_err(|_| "provider_unavailable".to_string())?;
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if deadline.saturating_duration_since(Instant::now()).is_zero() {
                cleanup_probe(&webview, &key_json);
                return Err("pin_snapshot_timeout".into());
            }
            let (tx, rx) = tokio::sync::oneshot::channel();
            let tx = Mutex::new(Some(tx));
            if webview
                .eval_with_callback(
                    format!("JSON.stringify(window[{key_json}]||null)"),
                    move |value| {
                        if let Ok(mut slot) = tx.lock() {
                            if let Some(tx) = slot.take() {
                                let _ = tx.send(value);
                            }
                        }
                    },
                )
                .is_err()
            {
                cleanup_probe(&webview, &key_json);
                return Err("provider_unavailable".into());
            }
            match tokio::time::timeout(Duration::from_secs(2), rx).await {
                Ok(Ok(raw)) => match parse_pin_snapshot_callback(&raw, &nonce, expected_scope) {
                    Some(PinSnapshotPoll::Pending) => {}
                    Some(PinSnapshotPoll::Complete(pinned_ids)) => {
                        let same_page = self.page.lock().ok().is_some_and(|page| {
                            page.navigation_generation == generation
                                && page.exact_origin
                                && page.account_scope.as_deref() == Some(expected_scope)
                        });
                        cleanup_probe(&webview, &key_json);
                        return if same_page {
                            Ok(ChatgptPinSnapshotOutcome {
                                complete: true,
                                account_scope: expected_scope.to_owned(),
                                pinned_ids,
                                error_code: None,
                                retry_after: None,
                            })
                        } else {
                            Err("chatgpt_page_changed".into())
                        };
                    }
                    Some(PinSnapshotPoll::Failed {
                        error_code,
                        retry_after,
                    }) => {
                        if error_code == "rate_limited" {
                            self.note_pin_snapshot_rate_limit(expected_scope, retry_after)?;
                        }
                        cleanup_probe(&webview, &key_json);
                        return Ok(ChatgptPinSnapshotOutcome {
                            complete: false,
                            account_scope: expected_scope.to_owned(),
                            pinned_ids: Vec::new(),
                            error_code: Some(error_code),
                            retry_after,
                        });
                    }
                    None => {
                        cleanup_probe(&webview, &key_json);
                        return Err("provider_unavailable".into());
                    }
                },
                Ok(Err(_)) => {
                    cleanup_probe(&webview, &key_json);
                    return Err("provider_unavailable".into());
                }
                Err(_) => {}
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    fn pin_snapshot_cooldown_remaining(
        &self,
        expected_scope: &str,
    ) -> Result<Option<Duration>, String> {
        let now = Instant::now();
        let mut deadlines = self
            .pin_snapshot_cooldowns
            .lock()
            .map_err(|_| "provider_unavailable".to_string())?;
        Ok(pin_cooldown_remaining(&mut deadlines, expected_scope, now))
    }

    fn note_pin_snapshot_rate_limit(
        &self,
        expected_scope: &str,
        retry_after: Option<u64>,
    ) -> Result<(), String> {
        let seconds = retry_after.unwrap_or(60).max(1);
        let now = Instant::now();
        let deadline = now
            .checked_add(Duration::from_secs(seconds))
            .unwrap_or_else(|| now + Duration::from_secs(31_536_001));
        let mut deadlines = self
            .pin_snapshot_cooldowns
            .lock()
            .map_err(|_| "provider_unavailable".to_string())?;
        extend_pin_cooldown(&mut deadlines, expected_scope, deadline);
        Ok(())
    }

    /// Rename one existing ChatGPT project using the captured project PATCH
    /// shape. The page verifies the account and reads back the changed record.
    pub async fn rename_project(
        &self,
        project_id: &str,
        name: &str,
        expected_scope: &str,
    ) -> Result<ProjectRenameOutcome, String> {
        if !valid_observed_scope(expected_scope)
            || project_id.len() > 132
            || !project_id.starts_with("g-p-")
            || !project_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
            || name.trim().is_empty()
            || name.chars().count() > 120
            || name.chars().any(char::is_control)
        {
            return Err("invalid_project_rename".into());
        }
        let (webview, loaded) = self
            .inner
            .lock()
            .map_err(|_| "provider_unavailable".to_string())
            .map(|inner| (inner.webview.clone(), inner.loaded))?;
        let generation = {
            let page = self
                .page
                .lock()
                .map_err(|_| "provider_unavailable".to_string())?;
            if !page.exact_origin {
                return Err("chatgpt_not_open".into());
            }
            if page.account_scope.as_deref() != Some(expected_scope) {
                return Err("chatgpt_account_changed".into());
            }
            page.navigation_generation
        };
        if !loaded {
            return Err("chatgpt_loading".into());
        }
        let nonce = fresh_probe_nonce().ok_or_else(|| "provider_unavailable".to_string())?;
        let key = format!("__chatHubProjectRename_{nonce}");
        let key_json = serde_json::to_string(&key).map_err(|_| "provider_unavailable")?;
        let nonce_json = serde_json::to_string(&nonce).map_err(|_| "provider_unavailable")?;
        let scope_json =
            serde_json::to_string(expected_scope).map_err(|_| "invalid_project_rename")?;
        let project_json =
            serde_json::to_string(project_id).map_err(|_| "invalid_project_rename")?;
        let name_json = serde_json::to_string(name.trim()).map_err(|_| "invalid_project_rename")?;
        let script = format!(
            "({})({key_json},{nonce_json},{scope_json},{project_json},{name_json})",
            include_str!("project_rename.js")
        );
        webview
            .eval(&script)
            .map_err(|_| "provider_unavailable".to_string())?;
        let deadline = Instant::now() + Duration::from_secs(120);
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                cleanup_probe(&webview, &key_json);
                return Err("project_rename_timeout".into());
            }
            let (tx, rx) = tokio::sync::oneshot::channel();
            let tx = Mutex::new(Some(tx));
            if webview
                .eval_with_callback(
                    format!("JSON.stringify(window[{key_json}]||null)"),
                    move |value| {
                        if let Ok(mut slot) = tx.lock() {
                            if let Some(tx) = slot.take() {
                                let _ = tx.send(value);
                            }
                        }
                    },
                )
                .is_err()
            {
                cleanup_probe(&webview, &key_json);
                return Err("provider_unavailable".into());
            }
            match tokio::time::timeout(remaining.min(Duration::from_secs(2)), rx).await {
                Ok(Ok(raw)) => match parse_project_rename_callback(&raw, &nonce) {
                    Some(RenamePoll::Pending) => {}
                    Some(RenamePoll::Complete(outcome)) => {
                        let same_page = self.page.lock().ok().is_some_and(|page| {
                            page.navigation_generation == generation
                                && page.exact_origin
                                && page.account_scope.as_deref() == Some(expected_scope)
                        });
                        cleanup_probe(&webview, &key_json);
                        return if same_page {
                            Ok(outcome)
                        } else {
                            Err("chatgpt_page_changed".into())
                        };
                    }
                    None => {
                        cleanup_probe(&webview, &key_json);
                        return Err("provider_unavailable".into());
                    }
                },
                Ok(Err(_)) => {
                    cleanup_probe(&webview, &key_json);
                    return Err("provider_unavailable".into());
                }
                Err(_) => {}
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    /// Apply a user-requested pin or archive mutation and require provider
    /// readback before returning a commit receipt to the local store.
    pub async fn set_conversation_state(
        &self,
        conversation_id: &str,
        action: &str,
        desired: bool,
        expected_scope: &str,
    ) -> Result<ConversationActionOutcome, String> {
        if !valid_conversation_id(conversation_id)
            || !valid_observed_scope(expected_scope)
            || !matches!(action, "archive" | "pin")
        {
            return Err("invalid_conversation_action".into());
        }
        let (webview, loaded) = self
            .inner
            .lock()
            .map_err(|_| "provider_unavailable".to_string())
            .map(|inner| (inner.webview.clone(), inner.loaded))?;
        let generation = {
            let page = self
                .page
                .lock()
                .map_err(|_| "provider_unavailable".to_string())?;
            if !page.exact_origin {
                return Err("chatgpt_not_open".into());
            }
            if page.account_scope.as_deref() != Some(expected_scope) {
                return Err("chatgpt_account_changed".into());
            }
            page.navigation_generation
        };
        if !loaded {
            return Err("chatgpt_loading".into());
        }
        let nonce = fresh_probe_nonce().ok_or_else(|| "provider_unavailable".to_string())?;
        let key = format!("__chatHubConversationAction_{nonce}");
        let key_json = serde_json::to_string(&key).map_err(|_| "provider_unavailable")?;
        let nonce_json = serde_json::to_string(&nonce).map_err(|_| "provider_unavailable")?;
        let scope_json =
            serde_json::to_string(expected_scope).map_err(|_| "invalid_conversation_action")?;
        let id_json =
            serde_json::to_string(conversation_id).map_err(|_| "invalid_conversation_action")?;
        let action_json =
            serde_json::to_string(action).map_err(|_| "invalid_conversation_action")?;
        let desired_json =
            serde_json::to_string(&desired).map_err(|_| "invalid_conversation_action")?;
        let script = format!(
            "({})({key_json},{nonce_json},{scope_json},{id_json},{action_json},{desired_json})",
            include_str!("conversation_actions.js")
        );
        webview
            .eval(&script)
            .map_err(|_| "provider_unavailable".to_string())?;
        let deadline = Instant::now() + Duration::from_secs(120);
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                cleanup_probe(&webview, &key_json);
                return Err("conversation_action_timeout".into());
            }
            let (tx, rx) = tokio::sync::oneshot::channel();
            let tx = Mutex::new(Some(tx));
            if webview
                .eval_with_callback(
                    format!("JSON.stringify(window[{key_json}]||null)"),
                    move |value| {
                        if let Ok(mut slot) = tx.lock() {
                            if let Some(tx) = slot.take() {
                                let _ = tx.send(value);
                            }
                        }
                    },
                )
                .is_err()
            {
                cleanup_probe(&webview, &key_json);
                return Err("provider_unavailable".into());
            }
            match tokio::time::timeout(remaining.min(Duration::from_secs(2)), rx).await {
                Ok(Ok(raw)) => match parse_conversation_action_callback(
                    &raw,
                    &nonce,
                    conversation_id,
                    action,
                    desired,
                ) {
                    Some(ActionPoll::Pending) => {}
                    Some(ActionPoll::Complete(outcome)) => {
                        let same_page = self.page.lock().ok().is_some_and(|page| {
                            page.navigation_generation == generation
                                && page.exact_origin
                                && page.account_scope.as_deref() == Some(expected_scope)
                        });
                        cleanup_probe(&webview, &key_json);
                        return if same_page {
                            Ok(outcome)
                        } else {
                            Err("chatgpt_page_changed".into())
                        };
                    }
                    None => {
                        cleanup_probe(&webview, &key_json);
                        return Err("provider_unavailable".into());
                    }
                },
                Ok(Err(_)) => {
                    cleanup_probe(&webview, &key_json);
                    return Err("provider_unavailable".into());
                }
                Err(_) => {}
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    /// Initialize the provider's own signed-in browser session in the hidden
    /// child webview. This never reveals the view or replaces a live page.
    pub fn prepare_session(&self) -> Result<bool, String> {
        #[cfg(feature = "native-smoke")]
        if crate::smoke() && !self.direct_smoke_enabled {
            // Ordinary isolated fixture runs must never contact the live site.
            return Ok(false);
        }
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| "provider_unavailable".to_string())?;
        if !should_prepare_session(inner.navigated) {
            return Ok(false);
        }
        inner
            .webview
            .navigate(tauri::Url::parse("https://chatgpt.com/").map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
        inner.loaded = false;
        inner.navigated = true;
        Ok(true)
    }

    pub(crate) fn history_webview(&self) -> Result<Webview<Wry>, String> {
        let (webview, loaded) = {
            let inner = self
                .inner
                .lock()
                .map_err(|_| "provider_unavailable".to_string())?;
            (inner.webview.clone(), inner.loaded)
        };
        let page = self
            .page
            .lock()
            .map_err(|_| "provider_unavailable".to_string())?;
        if !page.exact_origin || !loaded {
            return Err("provider_not_ready".into());
        }
        Ok(webview)
    }

    pub fn attach(
        app: &tauri::AppHandle,
        main: &WebviewWindow,
        proxy_url: &str,
        proxy_enabled: bool,
        direct_smoke_enabled: bool,
    ) -> Result<Webview<Wry>, String> {
        let initial_url = tauri::Url::parse("about:blank").map_err(|e| e.to_string())?;
        let app_handle = app.clone();
        let page = Arc::new(Mutex::new(PageMetadata::default()));
        let page_for_load = page.clone();
        let builder = WebviewBuilder::new("provider", tauri::WebviewUrl::External(initial_url))
            .on_navigation(allowed_provider_navigation)
            .on_page_load(move |_webview, payload| {
                let event = payload.event();
                if payload.url().scheme() != "https" {
                    if let Ok(mut metadata) = page_for_load.lock() {
                        if event == tauri::webview::PageLoadEvent::Started {
                            metadata.navigation_generation =
                                metadata.navigation_generation.saturating_add(1);
                            metadata.account_scope = None;
                            metadata.next_sequence = 0;
                            metadata.applied_sequence = 0;
                        }
                        metadata.exact_origin = false;
                        metadata.conversation_id = None;
                        metadata.account_scope = None;
                    }
                    if let Some(state) = app_handle.try_state::<ProviderViewState>() {
                        if let Ok(mut inner) = state.inner.lock() {
                            inner.loaded = event == tauri::webview::PageLoadEvent::Finished;
                        }
                    }
                    return;
                }
                if let Ok(mut metadata) = page_for_load.lock() {
                    if event == tauri::webview::PageLoadEvent::Started {
                        metadata.navigation_generation =
                            metadata.navigation_generation.saturating_add(1);
                        metadata.next_sequence = 0;
                        metadata.applied_sequence = 0;
                    }
                    metadata.exact_origin = exact_chatgpt_origin(payload.url());
                    metadata.conversation_id = current_chatgpt_conversation_id(payload.url());
                }
                if let Some(state) = app_handle.try_state::<ProviderViewState>() {
                    if let Ok(mut inner) = state.inner.lock() {
                        match event {
                            tauri::webview::PageLoadEvent::Started => inner.loaded = false,
                            tauri::webview::PageLoadEvent::Finished => inner.loaded = true,
                        }
                    }
                }
            });

        #[cfg(target_os = "macos")]
        let builder = if proxy_enabled {
            builder.proxy_url(tauri::Url::parse(proxy_url).map_err(|e| e.to_string())?)
        } else {
            builder
        };
        #[cfg(not(target_os = "macos"))]
        let builder = {
            let _ = (proxy_url, proxy_enabled);
            builder
        };

        #[cfg(all(target_os = "macos", feature = "native-smoke"))]
        let builder = if crate::smoke() {
            // Never reuse the owner's persistent WKWebsiteDataStore for synthetic
            // or direct smoke navigation. This random identifier is isolated to
            // the smoke process; production keeps WebKit's normal persistent store.
            let mut identifier = [0u8; 16];
            getrandom::getrandom(&mut identifier).map_err(|e| e.to_string())?;
            builder.data_store_identifier(identifier)
        } else {
            builder
        };
        #[cfg(not(all(target_os = "macos", feature = "native-smoke")))]
        let builder = builder;

        let webview = main
            .as_ref()
            .window()
            .add_child(
                builder,
                tauri::LogicalPosition::new(0.0, 0.0),
                tauri::LogicalSize::new(1.0, 1.0),
            )
            .map_err(|e| e.to_string())?;
        webview.hide().map_err(|e| e.to_string())?;
        app.manage(Self {
            inner: Mutex::new(Inner {
                webview: webview.clone(),
                navigated: false,
                loaded: false,
                visible: false,
            }),
            page,
            pin_snapshot_cooldowns: Mutex::new(HashMap::new()),
            proxy_enabled,
            direct_smoke_enabled,
        });
        Ok(webview)
    }

    pub fn set_view(
        &self,
        visible: bool,
        x: f64,
        y: f64,
        width: f64,
        height: f64,
        reload: bool,
    ) -> Result<ProviderViewStatus, String> {
        if ![x, y, width, height].iter().all(|n| n.is_finite())
            || (visible
                && (x < 0.0
                    || y < 0.0
                    || width < 1.0
                    || height < 1.0
                    || width > 20_000.0
                    || height > 20_000.0))
        {
            return Err("invalid provider view bounds".into());
        }

        let mut inner = self.inner.lock().map_err(|e| e.to_string())?;
        if !visible {
            inner.webview.hide().map_err(|e| e.to_string())?;
            inner.visible = false;
            let conversation_id = self
                .page
                .lock()
                .ok()
                .and_then(|page| page.conversation_id.clone());
            return Ok(ProviderViewStatus {
                state: "hidden".into(),
                message: None,
                conversation_id,
            });
        }

        inner
            .webview
            .set_position(tauri::LogicalPosition::new(x, y))
            .map_err(|e| e.to_string())?;
        inner
            .webview
            .set_size(tauri::LogicalSize::new(width, height))
            .map_err(|e| e.to_string())?;
        if reload {
            inner.loaded = false;
            inner.navigated = true;
            inner.webview.reload().map_err(|e| e.to_string())?;
        } else if !inner.navigated {
            // The app-scoped CA delegate is installed before this first navigation.
            inner.navigated = true;
            inner
                .webview
                .navigate(tauri::Url::parse("https://chatgpt.com/").map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())?;
        }
        inner.webview.show().map_err(|e| e.to_string())?;
        inner.visible = true;
        let ready = inner.loaded;
        let status = ProviderViewStatus {
            state: if ready { "ready" } else { "loading" }.into(),
            message: if self.proxy_enabled {
                None
            } else {
                Some("Live capture is off; ChatGPT is connected directly.".into())
            },
            conversation_id: self
                .page
                .lock()
                .ok()
                .and_then(|page| page.conversation_id.clone()),
        };
        let webview = inner.webview.clone();
        drop(inner);
        request_page_metadata(&webview, &self.page);
        Ok(status)
    }

    /// Navigate only after an explicit main-window command. Calling this does
    /// not change visibility or bounds, so the UI can keep its existing layout.
    pub fn open_conversation(&self, conversation_id: Option<&str>) -> Result<(), String> {
        let target = target_url(conversation_id)?;
        if let Ok(mut page) = self.page.lock() {
            page.navigation_generation = page.navigation_generation.saturating_add(1);
            page.account_scope = None;
            page.next_sequence = 0;
            page.applied_sequence = 0;
            page.exact_origin = true;
            page.conversation_id = current_chatgpt_conversation_id(&target);
        }
        let mut inner = self.inner.lock().map_err(|e| e.to_string())?;
        inner.loaded = false;
        inner.navigated = true;
        inner.webview.navigate(target).map_err(|e| e.to_string())
    }

    /// Check the current ChatGPT web session without navigating or returning
    /// credentials, account details, or provider response data to native code.
    pub async fn chatgpt_connection(&self) -> ChatGptConnectionStatus {
        let (webview, loaded) = match self.inner.lock() {
            Ok(inner) => (inner.webview.clone(), inner.loaded),
            Err(_) => return ChatGptConnectionStatus::new(ChatGptConnectionState::Unavailable),
        };
        let (page_is_chatgpt, page_generation) = match self.page.lock() {
            Ok(page) => (page.exact_origin, page.navigation_generation),
            Err(_) => return ChatGptConnectionStatus::new(ChatGptConnectionState::Unavailable),
        };
        if !page_is_chatgpt {
            return ChatGptConnectionStatus::new(ChatGptConnectionState::NotOpen);
        }
        if !loaded {
            return ChatGptConnectionStatus::new(ChatGptConnectionState::Loading);
        }

        let Some(nonce) = fresh_probe_nonce() else {
            return ChatGptConnectionStatus::new(ChatGptConnectionState::Unavailable);
        };
        let key = format!("__chatHubSessionProbe_{nonce}");
        let Ok(key_json) = serde_json::to_string(&key) else {
            return ChatGptConnectionStatus::new(ChatGptConnectionState::Unavailable);
        };
        let Ok(nonce_json) = serde_json::to_string(&nonce) else {
            return ChatGptConnectionStatus::new(ChatGptConnectionState::Unavailable);
        };
        let start_probe = format!(
            r#"(()=>{{const k={key_json};const n={nonce_json};window[k]={{nonce:n,state:"checking"}};(async()=>{{try{{const response=await fetch("/api/auth/session",{{credentials:"include",cache:"no-store"}});if(!response.ok){{window[k]={{nonce:n,state:response.status===401||response.status===403?"signed_out":"unavailable"}};return}}const session=await response.json();const token=session&&session.accessToken;if(typeof token!=="string"||!session.user){{window[k]={{nonce:n,state:"signed_out"}};return}}let scope=null;try{{const part=token.split(".")[1];if(!part||part.length>16000)throw 0;const claims=JSON.parse(atob(part.replace(/-/g,"+").replace(/_/g,"/")+"=".repeat((4-part.length%4)%4)));const auth=claims["https://api.openai.com/auth"];const account=auth?.chatgpt_account_id;let user=auth?.chatgpt_user_id;if(user==null)user=auth?.user_id;const valid=v=>typeof v==="string"&&v.length>0&&v.length<=256&&/^[A-Za-z0-9_:-]+$/.test(v);if(valid(account)&&valid(user)){{const bytes=new TextEncoder().encode(JSON.stringify(["chatgpt-request-identity-v1",account,user]));const digest=await crypto.subtle.digest("SHA-256",bytes);scope="chatgpt:observed:"+Array.from(new Uint8Array(digest),b=>b.toString(16).padStart(2,"0")).join("")}}}}catch(_){{scope=null}}window[k]={{nonce:n,state:"connected",account_scope:scope}}}}catch(_){{window[k]={{nonce:n,state:"unavailable"}}}}}})()}})()"#
        );
        if webview.eval(&start_probe).is_err() {
            return ChatGptConnectionStatus::new(ChatGptConnectionState::Unavailable);
        }

        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                cleanup_probe(&webview, &key_json);
                return ChatGptConnectionStatus::new(ChatGptConnectionState::Unavailable);
            }
            let (tx, rx) = tokio::sync::oneshot::channel();
            let tx = Mutex::new(Some(tx));
            let poll = format!("JSON.stringify(window[{key_json}]||null)");
            if webview
                .eval_with_callback(poll, move |value| {
                    if let Ok(mut slot) = tx.lock() {
                        if let Some(tx) = slot.take() {
                            let _ = tx.send(value);
                        }
                    }
                })
                .is_err()
            {
                cleanup_probe(&webview, &key_json);
                return ChatGptConnectionStatus::new(ChatGptConnectionState::Unavailable);
            }
            let wait = remaining.min(Duration::from_secs(2));
            match tokio::time::timeout(wait, rx).await {
                Ok(Ok(value)) => match parse_probe_callback(&value, &nonce) {
                    Some(ProbePoll::Pending) => {}
                    Some(ProbePoll::Complete(state, account_scope)) => {
                        cleanup_probe(&webview, &key_json);
                        return if state == ChatGptConnectionState::Connected {
                            let scoped = self.page.lock().ok().and_then(|mut page| {
                                if page.navigation_generation != page_generation
                                    || !page.exact_origin
                                {
                                    return None;
                                }
                                page.account_scope = account_scope.clone();
                                page.account_scope.clone()
                            });
                            ChatGptConnectionStatus::connected(scoped)
                        } else {
                            ChatGptConnectionStatus::new(state)
                        };
                    }
                    None => {
                        cleanup_probe(&webview, &key_json);
                        return ChatGptConnectionStatus::new(ChatGptConnectionState::Unavailable);
                    }
                },
                Ok(Err(_)) => {
                    cleanup_probe(&webview, &key_json);
                    return ChatGptConnectionStatus::new(ChatGptConnectionState::Unavailable);
                }
                Err(_) => {}
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    #[cfg(feature = "native-smoke")]
    pub fn set_smoke_view(&self, main: &Webview<Wry>) -> Result<(), String> {
        if !self.proxy_enabled && !self.direct_smoke_enabled {
            return Err("provider fixture proxy is not enabled".into());
        }
        let size = main.window().inner_size().map_err(|e| e.to_string())?;
        self.set_view(true, 0.0, 0.0, size.width as f64, size.height as f64, false)?;
        Ok(())
    }
}

#[cfg(test)]
mod target_url_tests {
    use super::{
        allowed_provider_navigation, current_chatgpt_conversation_id, exact_chatgpt_origin,
        extend_pin_cooldown, page_metadata_script, parse_assignment_callback,
        parse_conversation_action_callback, parse_page_metadata_callback,
        parse_pin_snapshot_callback, parse_probe_callback, parse_project_rename_callback,
        pin_cooldown_remaining, should_prepare_session, target_url, valid_observed_scope,
        ActionPoll, AssignmentPoll, ChatGptConnectionState, ConversationActionOutcome,
        PinSnapshotPoll, ProbePoll, ProjectRenameOutcome, RenamePoll,
    };
    use std::{
        collections::HashMap,
        time::{Duration, Instant},
    };

    #[test]
    fn conversation_action_callbacks_bind_nonce_target_and_verified_state() {
        let id = "123e4567-e89b-12d3-a456-426614174000";
        let applied = serde_json::json!({
            "nonce":"n1", "state":"applied", "cancelled":false,
            "verified":true, "conversation_id":id, "pinned":true
        });
        assert!(matches!(
            parse_conversation_action_callback(&applied.to_string(), "n1", id, "pin", true),
            Some(ActionPoll::Complete(ConversationActionOutcome {
                pinned: Some(true),
                ..
            }))
        ));
        let failed = serde_json::json!({
            "nonce":"n1", "state":"failed", "cancelled":false,
            "error_code":"rate_limited", "retry_after":30
        });
        assert!(
            matches!(parse_conversation_action_callback(&failed.to_string(), "n1", id, "archive", false), Some(ActionPoll::Complete(ConversationActionOutcome { error_code: Some(code), retry_after: Some(30), .. })) if code == "rate_limited")
        );
        let long_cooldown = serde_json::json!({
            "nonce":"n1", "state":"failed", "cancelled":false,
            "error_code":"rate_limited", "retry_after":31_536_001
        });
        assert!(matches!(
            parse_conversation_action_callback(&long_cooldown.to_string(), "n1", id, "pin", true),
            Some(ActionPoll::Complete(ConversationActionOutcome {
                retry_after: Some(31_536_001),
                ..
            }))
        ));
        for invalid in [
            serde_json::json!({"nonce":"other", "state":"applied", "cancelled":false,"verified":true,"conversation_id":id,"pinned":true}),
            serde_json::json!({"nonce":"n1", "state":"applied", "cancelled":false,"verified":true,"conversation_id":"123e4567-e89b-12d3-a456-426614174001","pinned":true}),
            serde_json::json!({"nonce":"n1", "state":"applied", "cancelled":false,"verified":true,"conversation_id":id,"pinned":false}),
            serde_json::json!({"nonce":"n1", "state":"applied", "cancelled":false,"verified":true,"conversation_id":id,"pinned":true,"archived":false}),
        ] {
            assert!(parse_conversation_action_callback(
                &invalid.to_string(),
                "n1",
                id,
                "pin",
                true
            )
            .is_none());
        }
    }

    #[test]
    fn pin_snapshot_callback_requires_complete_nonce_and_current_scope() {
        let scope = format!("chatgpt:observed:{}", "a".repeat(64));
        let id = "123e4567-e89b-12d3-a456-426614174000";
        let complete = serde_json::json!({
            "nonce":"pins-1", "state":"complete", "cancelled":false,
            "complete":true, "account_scope":scope, "pinned_ids":[id]
        });
        assert_eq!(
            parse_pin_snapshot_callback(&complete.to_string(), "pins-1", &scope),
            Some(PinSnapshotPoll::Complete(vec![id.to_owned()]))
        );
        let empty = serde_json::json!({
            "nonce":"pins-1", "state":"complete", "cancelled":false,
            "complete":true, "account_scope":scope, "pinned_ids":[]
        });
        assert_eq!(
            parse_pin_snapshot_callback(&empty.to_string(), "pins-1", &scope),
            Some(PinSnapshotPoll::Complete(Vec::new()))
        );
        let failed = serde_json::json!({
            "nonce":"pins-1", "state":"failed", "cancelled":false,
            "error_code":"rate_limited", "retry_after":3600
        });
        assert!(matches!(
            parse_pin_snapshot_callback(&failed.to_string(), "pins-1", &scope),
            Some(PinSnapshotPoll::Failed { error_code, retry_after: Some(3600) }) if error_code == "rate_limited"
        ));
        for invalid in [
            serde_json::json!({"nonce":"wrong","state":"complete","cancelled":false,"complete":true,"account_scope":scope,"pinned_ids":[id]}),
            serde_json::json!({"nonce":"pins-1","state":"complete","cancelled":false,"complete":false,"account_scope":scope,"pinned_ids":[id]}),
            serde_json::json!({"nonce":"pins-1","state":"complete","cancelled":false,"complete":true,"account_scope":"chatgpt:observed:{}","pinned_ids":[id]}),
            serde_json::json!({"nonce":"pins-1","state":"complete","cancelled":false,"complete":true,"account_scope":scope,"pinned_ids":[id,id]}),
            serde_json::json!({"nonce":"pins-1","state":"complete","cancelled":false,"complete":true,"account_scope":scope,"pinned_ids":["123E4567-E89B-12D3-A456-426614174000"]}),
            serde_json::json!({"nonce":"pins-1","state":"complete","cancelled":false,"complete":true,"account_scope":scope,"pinned_ids":[],"token":"secret"}),
        ] {
            assert!(parse_pin_snapshot_callback(&invalid.to_string(), "pins-1", &scope).is_none());
        }
    }

    #[test]
    fn pin_cooldown_is_scope_bound_and_never_shortened() {
        let now = Instant::now();
        let scope_a = format!("chatgpt:observed:{}", "a".repeat(64));
        let scope_b = format!("chatgpt:observed:{}", "b".repeat(64));
        let mut deadlines = HashMap::new();
        extend_pin_cooldown(&mut deadlines, &scope_a, now + Duration::from_secs(3600));
        extend_pin_cooldown(&mut deadlines, &scope_a, now + Duration::from_secs(30));
        assert_eq!(
            pin_cooldown_remaining(&mut deadlines, &scope_a, now),
            Some(Duration::from_secs(3600))
        );
        assert_eq!(pin_cooldown_remaining(&mut deadlines, &scope_b, now), None);
        assert_eq!(
            pin_cooldown_remaining(&mut deadlines, &scope_a, now + Duration::from_secs(3601)),
            None
        );
    }

    #[test]
    fn accepts_only_chatgpt_root_or_uuid_conversation() {
        assert_eq!(target_url(None).unwrap().as_str(), "https://chatgpt.com/");
        let id = "123e4567-e89b-12d3-a456-426614174000";
        assert_eq!(
            target_url(Some(id)).unwrap().as_str(),
            format!("https://chatgpt.com/c/{id}")
        );
        for bad in ["not-a-uuid", "https://example.com", "../settings", ""] {
            assert!(target_url(Some(bad)).is_err());
        }
    }

    #[test]
    fn background_session_preparation_only_navigates_an_unopened_provider() {
        assert!(should_prepare_session(false));
        assert!(!should_prepare_session(true));
        assert_eq!(target_url(None).unwrap().as_str(), "https://chatgpt.com/");
    }

    #[test]
    fn allows_https_login_redirects_but_rejects_local_or_script_navigation() {
        for url in [
            "about:blank",
            "https://chatgpt.com/",
            "https://login.microsoftonline.com/",
        ] {
            assert!(allowed_provider_navigation(
                &tauri::Url::parse(url).unwrap()
            ));
        }
        for url in [
            "http://chatgpt.com/",
            "file:///etc/hosts",
            "javascript:alert(1)",
            "data:text/html,hello",
        ] {
            assert!(!allowed_provider_navigation(
                &tauri::Url::parse(url).unwrap()
            ));
        }
    }

    #[test]
    fn probe_requires_exact_chatgpt_https_origin() {
        for url in ["https://chatgpt.com/", "https://chatgpt.com/c/abc"] {
            assert!(exact_chatgpt_origin(&tauri::Url::parse(url).unwrap()));
        }
        for url in [
            "about:blank",
            "http://chatgpt.com/",
            "https://chatgpt.com.evil.test/",
            "https://sub.chatgpt.com/",
            "https://user@chatgpt.com/",
            "https://login.microsoftonline.com/",
        ] {
            assert!(
                !exact_chatgpt_origin(&tauri::Url::parse(url).unwrap()),
                "{url}"
            );
        }
    }

    #[test]
    fn current_conversation_id_is_only_the_exact_canonical_chatgpt_uuid_path() {
        let id = "123e4567-e89b-12d3-a456-426614174000";
        assert_eq!(
            current_chatgpt_conversation_id(
                &tauri::Url::parse(&format!("https://chatgpt.com/c/{id}")).unwrap()
            ),
            Some(id.to_owned())
        );
        for url in [
            "https://chatgpt.com/",
            "https://chatgpt.com/c/not-a-uuid",
            "https://chatgpt.com/c/123E4567-E89B-12D3-A456-426614174000",
            "https://chatgpt.com/c/123e4567-e89b-12d3-a456-426614174000/extra",
            "https://chatgpt.com/c/123e4567-e89b-12d3-a456-426614174000?query=private",
            "https://chatgpt.com/c/123e4567-e89b-12d3-a456-426614174000#fragment",
            "https://chatgpt.com.evil.test/c/123e4567-e89b-12d3-a456-426614174000",
        ] {
            assert_eq!(
                current_chatgpt_conversation_id(&tauri::Url::parse(url).unwrap()),
                None,
                "{url}"
            );
        }
    }

    #[test]
    fn project_assignment_callback_accepts_only_nonce_bound_safe_receipts() {
        let applied = serde_json::json!({
            "nonce":"n1", "state":"applied", "cancelled":false,
            "project_title":"Research"
        })
        .to_string();
        assert_eq!(
            parse_assignment_callback(&applied, "n1"),
            Some(AssignmentPoll::Complete(super::ProjectAssignmentOutcome {
                state: "applied".into(),
                error_code: None,
                retry_after: None,
                project_title: Some("Research".into()),
            }))
        );
        let limited = serde_json::json!({
            "nonce":"n1", "state":"failed", "cancelled":false,
            "error_code":"rate_limited", "retry_after":7200
        })
        .to_string();
        assert!(
            matches!(parse_assignment_callback(&limited, "n1"), Some(AssignmentPoll::Complete(outcome)) if outcome.retry_after == Some(1800))
        );
        for invalid in [
            serde_json::json!({"nonce":"wrong","state":"running","cancelled":false}).to_string(),
            serde_json::json!({"nonce":"n1","state":"applied","project_title":"x","token":"secret"}).to_string(),
            serde_json::json!({"nonce":"n1","state":"failed","error_code":"request_failed","retry_after":4}).to_string(),
        ] {
            assert_eq!(parse_assignment_callback(&invalid, "n1"), None);
        }
    }

    #[test]
    fn project_rename_callback_requires_verified_nonce_bound_receipt() {
        let applied = serde_json::json!({
            "nonce":"rename-1", "state":"applied", "cancelled":false,
            "verified":true
        });
        assert!(matches!(
            parse_project_rename_callback(&applied.to_string(), "rename-1"),
            Some(RenamePoll::Complete(ProjectRenameOutcome {
                verified: true,
                ..
            }))
        ));
        let failed = serde_json::json!({
            "nonce":"rename-1", "state":"failed", "cancelled":false,
            "error_code":"verification_failed"
        });
        assert!(matches!(
            parse_project_rename_callback(&failed.to_string(), "rename-1"),
            Some(RenamePoll::Complete(ProjectRenameOutcome {
                verified: false,
                error_code: Some(code),
                ..
            })) if code == "verification_failed"
        ));
        for invalid in [
            serde_json::json!({
                "nonce":"other", "state":"applied", "cancelled":false,
                "verified":true
            }),
            serde_json::json!({
                "nonce":"rename-1", "state":"applied", "cancelled":false,
                "verified":false
            }),
            serde_json::json!({
                "nonce":"rename-1", "state":"applied", "cancelled":false,
                "verified":true, "project_name":"private"
            }),
        ] {
            assert!(parse_project_rename_callback(&invalid.to_string(), "rename-1").is_none());
        }
    }

    #[test]
    fn page_metadata_callback_returns_only_nonce_bound_origin_and_uuid() {
        let id = "123e4567-e89b-12d3-a456-426614174000";
        let safe = serde_json::json!({
            "nonce": "test-nonce", "exact_origin": true, "conversation_id": id
        })
        .to_string();
        assert_eq!(
            parse_page_metadata_callback(&safe, "test-nonce"),
            Some((true, Some(id.to_owned())))
        );
        let nested = serde_json::to_string(&safe).unwrap();
        assert_eq!(
            parse_page_metadata_callback(&nested, "test-nonce"),
            Some((true, Some(id.to_owned())))
        );
        let root = serde_json::json!({
            "nonce": "test-nonce", "exact_origin": true, "conversation_id": null
        })
        .to_string();
        assert_eq!(
            parse_page_metadata_callback(&root, "test-nonce"),
            Some((true, None))
        );
        for raw in [
            r#"{"nonce":"wrong","exact_origin":true,"conversation_id":null}"#,
            r#"{"nonce":"test-nonce","exact_origin":false,"conversation_id":"123e4567-e89b-12d3-a456-426614174000"}"#,
            r#"{"nonce":"test-nonce","exact_origin":true,"conversation_id":"invalid"}"#,
            r#"{"nonce":"test-nonce","exact_origin":true,"conversation_id":null,"url":"https://chatgpt.com/c/private"}"#,
        ] {
            assert_eq!(parse_page_metadata_callback(raw, "test-nonce"), None);
        }
    }

    #[test]
    fn page_location_probe_returns_only_safe_metadata_and_checks_exact_uuid_route() {
        let script = page_metadata_script("safe-nonce").unwrap();
        assert!(script.contains("new URL(location.href)"));
        assert!(script.contains("hostname === 'chatgpt.com'"));
        assert!(script.contains("conversation_id: conversationId"));
        assert!(!script.contains("accessToken"));
        assert!(!script.contains("title"));
    }

    #[test]
    fn parses_only_nonce_bound_status_from_callback_shapes() {
        let nonce = "001122";
        let signed_out = serde_json::json!({"nonce": nonce, "state": "signed_out"}).to_string();
        let signed_out_nested = serde_json::to_string(&signed_out).unwrap();
        let signed_out_deep_nested = serde_json::to_string(&signed_out_nested).unwrap();
        for (raw, expected) in [
            (
                r#"{"nonce":"001122","state":"checking"}"#,
                ProbePoll::Pending,
            ),
            (
                r#""{\"nonce\":\"001122\",\"state\":\"connected\"}""#,
                ProbePoll::Complete(ChatGptConnectionState::Connected, None),
            ),
            (
                r#"{"nonce":"001122","state":"unavailable"}"#,
                ProbePoll::Complete(ChatGptConnectionState::Unavailable, None),
            ),
        ] {
            assert_eq!(parse_probe_callback(raw, nonce), Some(expected));
        }
        assert_eq!(
            parse_probe_callback(&signed_out_deep_nested, nonce),
            Some(ProbePoll::Complete(ChatGptConnectionState::SignedOut, None))
        );
        for raw in [
            r#"{"nonce":"wrong","state":"connected"}"#,
            r#"{"nonce":"001122","state":"connected","accessToken":"secret"}"#,
            r#"{"nonce":"001122","state":"unknown"}"#,
            "not-json",
        ] {
            assert_eq!(parse_probe_callback(raw, nonce), None);
        }
    }

    #[test]
    fn connection_probe_accepts_only_opaque_observed_scope() {
        let scope = format!("chatgpt:observed:{}", "a".repeat(64));
        let raw = serde_json::json!({"nonce":"001122", "state":"connected", "account_scope":scope})
            .to_string();
        assert_eq!(
            parse_probe_callback(&raw, "001122"),
            Some(ProbePoll::Complete(
                ChatGptConnectionState::Connected,
                Some(format!("chatgpt:observed:{}", "a".repeat(64))),
            ))
        );
        assert!(!valid_observed_scope("chatgpt:observed:secret"));
        assert!(parse_probe_callback(
            r#"{"nonce":"001122","state":"connected","account_scope":"raw-account-id"}"#,
            "001122"
        )
        .is_none());
        assert!(parse_probe_callback(
            r#"{"nonce":"001122","state":"signed_out","account_scope":"chatgpt:observed:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}"#,
            "001122"
        ).is_none());
    }
}
