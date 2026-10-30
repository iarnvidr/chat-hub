use chat_hub::{
    config::{Config, APP_NAME},
    runtime::RuntimeFiles,
    service,
};
use std::{env, time::Duration};
#[tokio::main]
async fn main() {
    if let Err(e) = run().await {
        eprintln!("{APP_NAME}: {e}");
        std::process::exit(1)
    }
}
async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = env::args().skip(1);
    match args.next().as_deref() {
        Some("status") => {
            let c = Config::from_env()?;
            let s = chat_hub::runtime::status(&c).await?;
            println!("{}", serde_json::to_string(&s)?);
            Ok(())
        }
        Some("search") => {
            let q = args.collect::<Vec<_>>().join(" ");
            let c = Config::from_env()?;
            for id in chat_hub::store::Store::open(&c.database_path)?.search(&q, 50)? {
                println!("{id}")
            }
            Ok(())
        }
        Some("read") => {
            let id = args.next().ok_or("usage: chat-hub read <id>")?;
            let c = Config::from_env()?;
            if let Some(v) = chat_hub::store::Store::open(&c.database_path)?.read(&id)? {
                println!("{}", serde_json::to_string_pretty(&v)?);
                Ok(())
            } else {
                Err("conversation not found".into())
            }
        }
        Some("serve") => {
            let c = Config::from_env()?;
            let (handle, _capture_sender) = service::start(c.clone(), 128).await?;
            let st = handle.status();
            let runtime = RuntimeFiles::create(
                &c,
                st.api_addr.ok_or("API address unavailable")?,
                st.proxy_addr.ok_or("proxy address unavailable")?,
                &st.identity,
            )
            .map_err(|e| format!("runtime setup: {e}"))?;
            println!("{{\"service\":\"{APP_NAME}\",\"status\":\"running\",\"pid\":{},\"instance_id\":\"{}\",\"api_addr\":\"{}\",\"proxy_addr\":\"{}\"}}",st.identity.pid,st.identity.instance_id,st.api_addr.unwrap(),st.proxy_addr.unwrap());
            tokio::signal::ctrl_c().await?;
            let stats = handle.shutdown_timeout(Duration::from_secs(5)).await;
            runtime.cleanup();
            eprintln!(
                "shutdown complete: stored={}, ignored={}, failed={}, unprocessed={}",
                stats.stored, stats.ignored, stats.failed, stats.unprocessed
            );
            Ok(())
        }
        _ => Err("usage: chat-hub <status|search|read|serve>".into()),
    }
}
