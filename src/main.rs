use chat_hub::{
    config::{Config, APP_NAME},
    server,
    store::Store,
};
use std::env;

#[tokio::main]
async fn main() {
    let result = run().await;
    if let Err(error) = result {
        eprintln!("{APP_NAME}: {error}");
        std::process::exit(1);
    }
}
async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = env::args().skip(1);
    match args.next().as_deref() {
        Some("status") => {
            let c = Config::from_env()?;
            let s = Store::open(&c.database_path)?;
            println!(
                "{{\"service\":\"{APP_NAME}\",\"status\":\"{}\"}}",
                if s.health()? { "ok" } else { "unavailable" }
            );
            Ok(())
        }
        Some("search") => {
            let q = args.collect::<Vec<_>>().join(" ");
            let c = Config::from_env()?;
            for id in Store::open(&c.database_path)?.search(&q, 50)? {
                println!("{id}");
            }
            Ok(())
        }
        Some("read") => {
            let id = args.next().ok_or("usage: chat-hub read <id>")?;
            let c = Config::from_env()?;
            if let Some(v) = Store::open(&c.database_path)?.read(&id)? {
                println!("{}", serde_json::to_string_pretty(&v)?);
            } else {
                return Err("conversation not found".into());
            }
            Ok(())
        }
        Some("serve") => {
            let c = Config::from_env()?;
            let (tx, mut rx) = tokio::sync::mpsc::channel(128);
            tokio::spawn(async move {
                let mut count: u64 = 0;
                while rx.recv().await.is_some() {
                    count += 1;
                    if count == 1 {
                        eprintln!("capture events received (content omitted)");
                    }
                }
            });
            let proxy_config = chat_hub::proxy::ProxyConfig {
                ca_dir: c.ca_dir.clone(),
                bind: c.proxy_addr,
                test_upstream: None,
                test_roots: None,
            };
            let (proxy, _proxy_task) = chat_hub::proxy::run(proxy_config, tx).await?;
            let ca_path = c.ca_dir.join("ca.pem").display().to_string();
            let listener = tokio::net::TcpListener::bind(c.bind_addr).await?;
            let api_addr = listener.local_addr()?;
            println!(
                "API listening on {api_addr}; proxy listening on {}; CA certificate: {ca_path}",
                proxy.addr
            );
            axum::serve(
                listener,
                server::router(Store::open(&c.database_path)?, c.bearer_token),
            )
            .await?;
            Ok(())
        }
        _ => Err("usage: chat-hub <status|search|read|serve>".into()),
    }
}
