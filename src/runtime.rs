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
        r.write(c)?;
        Ok(r)
    }
    fn write(&self, c: &Config) -> Result<(), String> {
        for (name, data) in [
            (
                "service.json",
                serde_json::to_vec(&self.record).map_err(|e| e.to_string())?,
            ),
            ("service.token", c.bearer_token.as_bytes().to_vec()),
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
            fs::rename(&tmp, path).map_err(|e| {
                let _ = fs::remove_file(&tmp);
                format!("publish runtime file: {e}")
            })?;
        }
        Ok(())
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
