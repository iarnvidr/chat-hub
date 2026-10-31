use chat_hub::{
    config::{Config, APP_NAME},
    runtime::RuntimeFiles,
    service,
};
use std::{env, time::Duration};

const HELP: &str = "Chat Hub — local conversation library

Usage: chat-hub <command> [arguments]

Commands:
  status                       Check the desktop app or standalone service and library
  search <words...>            Search saved messages; print matching chat IDs
  read <id>                   Print a saved conversation as JSON
  import <export.json>        Import a ChatGPT export into the local library
  backup <destination>        Save a SQLite library backup
  export-json <destination>   Export the local library as JSON
  restore <backup.sqlite3>    Restore into an empty library
  serve                       Run the standalone local service (CHAT_HUB_TOKEN required)

Options:
  -h, --help                  Show this help
  -V, --version               Show the version

Uses the desktop app's library by default. CHAT_HUB_DATABASE selects another library.
Local library commands do not require a token. ChatGPT sign-in and live chat use the app.";
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
        None | Some("-h" | "--help" | "help") => {
            println!("{HELP}");
            Ok(())
        }
        Some("-V" | "--version" | "version") => {
            println!("{APP_NAME} {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Some("import") => {
            let path = args.next().ok_or("usage: chat-hub import <path>")?;
            if args.next().is_some() {
                return Err("usage: chat-hub import <path>".into());
            }
            let file = std::fs::File::open(path).map_err(|_| "unable to open import input")?;
            let bytes = chat_hub::importer::read_input(file)?;
            // Validate before opening the database or creating its data directory.
            chat_hub::importer::parse_export(&bytes)?;
            let c = Config::for_local_cli()?;
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
        Some("restore") => {
            let source = args
                .next()
                .ok_or("usage: chat-hub restore <backup.sqlite3>")?;
            if args.next().is_some() {
                return Err("usage: chat-hub restore <backup.sqlite3>".into());
            }
            let c = Config::for_local_cli()?;
            if c.database_path == ":memory:" {
                return Err("restore requires a file-backed library".into());
            }
            if let Some(parent) = std::path::Path::new(&c.database_path)
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
            {
                std::fs::create_dir_all(parent)
                    .map_err(|_| "unable to create library directory")?;
            }
            let mut store = chat_hub::store::Store::open(&c.database_path)
                .map_err(|_| "unable to open restore destination")?;
            let receipt = store.restore_from(source)?;
            println!("{{\"operation\":\"restore\",\"schema_version\":{},\"library_revision\":{},\"conversation_count\":{}}}", receipt.schema_version, receipt.library_revision, receipt.conversation_count);
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
            let c = Config::for_local_cli()?;
            if c.database_path == ":memory:" {
                return Err("archive commands require an existing file-backed library".into());
            }
            let store = chat_hub::store::Store::open_existing(&c.database_path)
                .map_err(|_| "unable to open existing library")?;
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
            if args.next().is_some() {
                return Err("usage: chat-hub status".into());
            }
            let c = Config::for_local_cli()?;
            let s = chat_hub::runtime::status(&c).await?;
            println!("{}", serde_json::to_string(&s)?);
            Ok(())
        }
        Some("search") => {
            let q = args.collect::<Vec<_>>().join(" ");
            if q.trim().is_empty() {
                return Err("usage: chat-hub search <words...>".into());
            }
            let c = Config::for_local_cli()?;
            for id in chat_hub::store::Store::open_existing(&c.database_path)?.search(&q, 50)? {
                println!("{id}")
            }
            Ok(())
        }
        Some("read") => {
            let id = args.next().ok_or("usage: chat-hub read <id>")?;
            if args.next().is_some() {
                return Err("usage: chat-hub read <id>".into());
            }
            let c = Config::for_local_cli()?;
            if let Some(v) = chat_hub::store::Store::open_existing(&c.database_path)?.read(&id)? {
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
            let capture = handle.capture_status();
            let stats = handle.shutdown_timeout(Duration::from_secs(5)).await;
            runtime.cleanup();
            eprintln!("{}", service::shutdown_report(&stats, capture.snapshot()));
            Ok(())
        }
        _ => Err("unknown command; run chat-hub --help".into()),
    }
}
