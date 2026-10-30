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
        Some("import") => {
            let path = args.next().ok_or("usage: chat-hub import <path>")?;
            if args.next().is_some() {
                return Err("usage: chat-hub import <path>".into());
            }
            let file = std::fs::File::open(path).map_err(|_| "unable to open import input")?;
            let bytes = chat_hub::importer::read_input(file)?;
            // Validate before opening the database or creating its data directory.
            chat_hub::importer::parse_export(&bytes)?;
            let c = Config::from_env()?;
            if c.database_path != ":memory:" {
                if let Some(parent) = std::path::Path::new(&c.database_path)
                    .parent()
                    .filter(|p| !p.as_os_str().is_empty())
                {
                    std::fs::create_dir_all(parent)
                        .map_err(|_| "unable to create import library directory")?;
                }
            }
            let mut store = chat_hub::store::Store::open(&c.database_path)
                .map_err(|_| "unable to open import library")?;
            let receipt = chat_hub::importer::import_chatgpt(&mut store, &bytes)?;
            println!("{}", serde_json::to_string(&receipt)?);
            Ok(())
        }
        Some(command @ ("backup" | "export-json")) => {
            let usage = if command == "backup" {
                "usage: chat-hub backup <destination>"
            } else {
                "usage: chat-hub export-json <destination>"
            };
            let destination = args.next().ok_or(usage)?;
            if args.next().is_some() {
                return Err(usage.into());
            }
            let c = Config::from_env()?;
            if c.database_path != ":memory:" {
                if let Some(parent) = std::path::Path::new(&c.database_path)
                    .parent()
                    .filter(|p| !p.as_os_str().is_empty())
                {
                    std::fs::create_dir_all(parent)
                        .map_err(|_| "unable to create library directory")?;
                }
            }
            let store = chat_hub::store::Store::open(&c.database_path)
                .map_err(|_| "unable to open library")?;
            if command == "backup" {
                let receipt = store.backup_to(&destination)?;
                println!("{{\"operation\":\"backup\",\"schema_version\":{},\"library_revision\":{},\"conversation_count\":{}}}", receipt.schema_version, receipt.library_revision, receipt.conversation_count);
            } else {
                let receipt = store.export_json_to(&destination)?;
                let m = receipt.metadata;
                println!("{{\"operation\":\"export-json\",\"schema_version\":{},\"library_revision\":{},\"conversation_count\":{},\"node_count\":{},\"message_count\":{},\"observation_count\":{}}}", m.schema_version, m.library_revision, m.conversation_count, m.node_count, m.message_count, m.observation_count);
            }
            Ok(())
        }
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
        _ => Err("usage: chat-hub <status|search|read|serve|import|backup|export-json>".into()),
    }
}
