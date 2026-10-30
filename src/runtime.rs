use crate::config::Config;
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    net::SocketAddr,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const MAX_RECORD: u64 = 16 * 1024;
const MAX_TOKEN: u64 = 4096;
const MAX_HEALTH_BODY: usize = 16 * 1024;
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ServiceRecord {
    pub schema: u32,
    pub pid: u32,
    pub api_addr: String,
    pub proxy_addr: String,
    pub started_unix_ms: u128,
    pub instance_id: String,
}

pub struct RuntimeFiles {
    dir: PathBuf,
    record: ServiceRecord,
    lock: PathBuf,
    lock_owned: bool,
}
impl RuntimeFiles {
    pub fn create(
        c: &Config,
        api: SocketAddr,
        proxy: SocketAddr,
        identity: &crate::server::RuntimeIdentity,
    ) -> Result<Self, String> {
        Self::create_with_publish_hook(c, api, proxy, identity, |_| Ok(()))
    }

    fn create_with_publish_hook(
        c: &Config,
        api: SocketAddr,
        proxy: SocketAddr,
        identity: &crate::server::RuntimeIdentity,
        before_publish: impl FnMut(&str) -> Result<(), String>,
    ) -> Result<Self, String> {
        let dir = Path::new(&c.database_path)
            .parent()
            .unwrap_or(Path::new("."))
            .join("runtime");
        fs::create_dir_all(&dir).map_err(|e| format!("create runtime directory: {e}"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&dir, fs::Permissions::from_mode(0o700))
                .map_err(|e| format!("secure runtime directory: {e}"))?;
        }
        let lock = dir.join("service.lock");
        let mut opts = OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut f = opts.open(&lock).map_err(|e| if e.kind() == std::io::ErrorKind::AlreadyExists { "service runtime lock exists; another service may be running. Verify no chat-hub service is active, then remove runtime/service.lock and retry".to_string() } else { format!("create service runtime lock: {e}") })?;
        let r = Self {
            dir,
            record: ServiceRecord {
                schema: 1,
                pid: std::process::id(),
                api_addr: api.to_string(),
                proxy_addr: proxy.to_string(),
                started_unix_ms: SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis(),
                instance_id: identity.instance_id.clone(),
            },
            lock,
            lock_owned: true,
        };
        if let Err(e) = writeln!(f, "{}", r.record.pid).and_then(|_| f.sync_all()) {
            return Err(format!("write service runtime lock: {e}"));
        }
        r.write(c, before_publish)?;
        Ok(r)
    }
    fn write(
        &self,
        c: &Config,
        mut before_publish: impl FnMut(&str) -> Result<(), String>,
    ) -> Result<(), String> {
        let mut token_published = false;
        let result = (|| {
            // The discovery record is the final publication marker: a reader must
            // never discover this instance before its credential is available.
            for (name, data) in [
                ("service.token", c.bearer_token.as_bytes().to_vec()),
                (
                    "service.json",
                    serde_json::to_vec(&self.record).map_err(|e| e.to_string())?,
                ),
            ] {
                let tmp = self.dir.join(format!("{name}.tmp.{}", self.record.pid));
                let path = self.dir.join(name);
                let mut o = OpenOptions::new();
                o.write(true).create_new(true);
                #[cfg(unix)]
                {
                    use std::os::unix::fs::OpenOptionsExt;
                    o.mode(0o600);
                }
                let mut f = o
                    .open(&tmp)
                    .map_err(|e| format!("create runtime file: {e}"))?;
                f.write_all(&data).and_then(|_| f.sync_all()).map_err(|e| {
                    let _ = fs::remove_file(&tmp);
                    format!("write runtime file: {e}")
                })?;
                if let Err(error) = before_publish(name) {
                    let _ = fs::remove_file(&tmp);
                    return Err(error);
                }
                fs::rename(&tmp, path).map_err(|e| {
                    let _ = fs::remove_file(&tmp);
                    format!("publish runtime file: {e}")
                })?;
                if name == "service.token" {
                    token_published = true;
                }
            }
            Ok(())
        })();
        // With no discovery marker, remove only the credential we just wrote.
        // Preserve an externally replaced credential; lock cleanup stays owned
        // by RuntimeFiles::drop and the existing record-identity check.
        if result.is_err() && token_published {
            let path = self.dir.join("service.token");
            if fs::read(&path).is_ok_and(|bytes| bytes == c.bearer_token.as_bytes()) {
                let _ = fs::remove_file(path);
            }
        }
        result
    }
    fn matches(&self) -> bool {
        read_record(&self.dir.join("service.json")).is_some_and(|r| {
            r.pid == self.record.pid
                && r.started_unix_ms == self.record.started_unix_ms
                && r.instance_id == self.record.instance_id
        })
    }
    pub fn cleanup(mut self) {
        self.remove_owned();
    }
    fn remove_owned(&mut self) {
        if self.lock_owned {
            if self.matches() {
                let _ = fs::remove_file(self.dir.join("service.json"));
                let _ = fs::remove_file(self.dir.join("service.token"));
            }
            let _ = fs::remove_file(&self.lock);
            self.lock_owned = false;
        }
    }
}
impl Drop for RuntimeFiles {
    fn drop(&mut self) {
        self.remove_owned();
    }
}

fn read_record(path: &Path) -> Option<ServiceRecord> {
    let m = fs::metadata(path).ok()?;
    if m.len() > MAX_RECORD {
        return None;
    }
    let record: ServiceRecord = serde_json::from_slice(&fs::read(path).ok()?).ok()?;
    let api = record.api_addr.parse::<SocketAddr>().ok()?;
    let proxy = record.proxy_addr.parse::<SocketAddr>().ok()?;
    if record.schema != 1
        || record.pid == 0
        || !api.ip().is_loopback()
        || !proxy.ip().is_loopback()
        || record.instance_id.is_empty()
    {
        return None;
    }
    Some(record)
}

async fn read_bounded_health_body(
    mut response: reqwest::Response,
) -> Result<serde_json::Value, String> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(1);
    let mut body = Vec::new();
    loop {
        let chunk = tokio::time::timeout_at(deadline, response.chunk())
            .await
            .map_err(|_| "health response body timed out".to_string())?
            .map_err(|e| e.to_string())?;
        let Some(chunk) = chunk else { break };
        if chunk.len() > MAX_HEALTH_BODY.saturating_sub(body.len()) {
            return Err("health response body too large".into());
        }
        body.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&body).map_err(|e| e.to_string())
}
pub async fn status(c: &Config) -> Result<serde_json::Value, String> {
    let dir = Path::new(&c.database_path)
        .parent()
        .unwrap_or(Path::new("."))
        .join("runtime");
    let record = read_record(&dir.join("service.json"));
    let db = c.database_path.clone();
    let dbhealth = tokio::task::spawn_blocking(move || {
        if !Path::new(&db).exists() {
            return Ok(None);
        }
        if fs::metadata(&db).map(|m| m.len()).unwrap_or(0) == 0 {
            return Ok(None);
        }
        crate::store::Store::open(&db).and_then(|s| s.health().map(Some))
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())?;
    let mut running = false;
    let mut identity_mismatch = false;
    if let Some(r) = &record {
        if let Ok(addr) = r.api_addr.parse::<SocketAddr>() {
            if addr.ip().is_loopback() {
                if let Ok(meta) = fs::metadata(dir.join("service.token")) {
                    if meta.len() <= MAX_TOKEN && meta.len() > 0 {
                        if let Ok(tok) = fs::read(dir.join("service.token")) {
                            if let Ok(tok) = std::str::from_utf8(&tok) {
                                let client = reqwest::Client::builder()
                                    .no_proxy()
                                    .redirect(reqwest::redirect::Policy::none())
                                    .timeout(Duration::from_secs(1))
                                    .build()
                                    .map_err(|e| e.to_string())?;
                                if let Ok(response) = client
                                    .get(format!("http://{addr}/api/health"))
                                    .bearer_auth(tok)
                                    .send()
                                    .await
                                {
                                    if response.status().is_success() {
                                        if let Ok(body) = read_bounded_health_body(response).await {
                                            running = body["pid"].as_u64()
                                                == Some(u64::from(r.pid))
                                                && body["instance_id"].as_str()
                                                    == Some(r.instance_id.as_str());
                                            identity_mismatch = !running;
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    Ok(
        serde_json::json!({"schema":1,"database_health":dbhealth,"service":if running{"running"}else if identity_mismatch{"identity_mismatch"}else{"stopped"},"pid":record.as_ref().map(|r|r.pid),"instance_id":record.as_ref().map(|r|&r.instance_id),"api_addr":record.as_ref().map(|r|&r.api_addr),"proxy_addr":record.as_ref().map(|r|&r.proxy_addr)}),
    )
}

#[cfg(test)]
mod publication_tests {
    use super::*;
    use crate::server::RuntimeIdentity;

    fn config() -> Config {
        let dir = std::env::temp_dir().join(format!(
            "chat-hub-runtime-publication-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        Config {
            capture_enabled: false,
            bind_addr: "127.0.0.1:41001".parse().unwrap(),
            proxy_addr: "127.0.0.1:41002".parse().unwrap(),
            bearer_token: "synthetic-publication-token".into(),
            database_path: dir.join("db.sqlite").to_string_lossy().into_owned(),
            ca_dir: dir.join("ca"),
        }
    }
    fn identity() -> RuntimeIdentity {
        RuntimeIdentity {
            pid: std::process::id(),
            instance_id: "synthetic-publication-instance".into(),
        }
    }

    #[test]
    fn actual_publisher_hides_record_until_credential_is_published() {
        let config = config();
        let dir = Path::new(&config.database_path)
            .parent()
            .unwrap()
            .join("runtime");
        let (blocked, entered) = std::sync::mpsc::channel();
        let (release, resume) = std::sync::mpsc::channel();
        let publisher_config = config.clone();
        let publisher = std::thread::spawn(move || {
            RuntimeFiles::create_with_publish_hook(
                &publisher_config,
                publisher_config.bind_addr,
                publisher_config.proxy_addr,
                &identity(),
                |name| {
                    if name == "service.token" {
                        blocked.send(()).unwrap();
                        resume
                            .recv_timeout(Duration::from_secs(10))
                            .expect("publisher barrier was not released");
                    }
                    Ok(())
                },
            )
        });
        entered
            .recv_timeout(Duration::from_secs(5))
            .expect("publisher did not reach credential barrier");
        // This barrier is inside the actual file publisher, immediately before
        // credential rename. No timing assumption, retry, or sleep is involved.
        let hidden = !dir.join("service.json").exists();
        let token_hidden = !dir.join("service.token").exists();
        let lock_present = dir.join("service.lock").exists();
        release.send(()).unwrap();
        let runtime = publisher.join().unwrap().unwrap();
        assert!(hidden, "discovery record preceded its credential");
        assert!(token_hidden);
        assert!(lock_present);
        let record = read_record(&dir.join("service.json")).unwrap();
        assert_eq!(record.instance_id, identity().instance_id);
        assert_eq!(record.pid, identity().pid);
        assert_eq!(
            fs::read(dir.join("service.token")).unwrap(),
            config.bearer_token.as_bytes()
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for name in ["service.json", "service.token", "service.lock"] {
                assert_eq!(
                    fs::metadata(dir.join(name)).unwrap().permissions().mode() & 0o777,
                    0o600
                );
            }
            assert_eq!(
                fs::metadata(&dir).unwrap().permissions().mode() & 0o777,
                0o700
            );
        }
        runtime.cleanup();
        for name in ["service.json", "service.token", "service.lock"] {
            assert!(!dir.join(name).exists());
        }
        fs::remove_dir_all(dir.parent().unwrap()).unwrap();
    }

    #[test]
    fn failure_before_discovery_marker_leaves_no_published_readiness_or_owned_credential() {
        for fail_at in ["service.token", "service.json"] {
            let config = config();
            let dir = Path::new(&config.database_path)
                .parent()
                .unwrap()
                .join("runtime");
            let result = RuntimeFiles::create_with_publish_hook(
                &config,
                config.bind_addr,
                config.proxy_addr,
                &identity(),
                |name| {
                    if name == fail_at {
                        return Err("synthetic publication failure".into());
                    }
                    Ok(())
                },
            );
            assert!(result.is_err());
            assert!(read_record(&dir.join("service.json")).is_none());
            assert!(!dir.join("service.token").exists());
            assert!(!dir.join("service.lock").exists());
            assert_eq!(fs::read_dir(&dir).unwrap().count(), 0);
            fs::remove_dir_all(dir.parent().unwrap()).unwrap();
        }
    }
}
