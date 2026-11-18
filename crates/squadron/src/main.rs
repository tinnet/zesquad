use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::process::ExitCode;
use std::time::Duration;

use clap::Parser as _;
use squadron::cli::{BnetArgs, Cli, Command, ServeArgs};
use squadron::{Settings, armory};
use tracing_subscriber::EnvFilter;

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[tokio::main]
async fn main() -> ExitCode {
    // A missing .env is normal (containers use real environment variables).
    let _ = dotenvy::dotenv();
    let cli = Cli::parse();
    let result = match cli.command {
        Command::Serve(args) => serve(args).await,
        Command::Healthcheck { listen } => return healthcheck(listen).await,
        Command::Api {
            path,
            namespace,
            bnet,
        } => api(&path, &namespace, &bnet).await,
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("squadron: {e:#}");
            ExitCode::FAILURE
        }
    }
}

async fn serve(args: ServeArgs) -> anyhow::Result<()> {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let logs = tracing_subscriber::fmt().with_env_filter(filter);
    if args.log_json {
        logs.json().init();
    } else {
        logs.init();
    }
    let settings = Settings {
        listen: args.listen,
        database: args.database.clone(),
        refresh: args.refresh(),
        region: args.bnet.region.clone(),
        armory: args.bnet.config()?,
        password: args.password.clone(),
    };
    let running = squadron::start(settings, squadron::shutdown_signal()).await?;
    tracing::info!(
        version = squadron::version::short(),
        "listening on http://{}",
        running.addr
    );
    running.server.await?
}

/// Exit code only, no output: it runs every few seconds in a container.
async fn healthcheck(listen: SocketAddr) -> ExitCode {
    // A server on 0.0.0.0 is reached on loopback.
    let host = if listen.ip().is_unspecified() {
        IpAddr::V4(Ipv4Addr::LOCALHOST)
    } else {
        listen.ip()
    };
    let url = format!("http://{}/healthz", SocketAddr::new(host, listen.port()));
    let healthy = async {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(3))
            .build()?;
        anyhow::Ok(client.get(url).send().await?.status().is_success())
    };
    match healthy.await {
        Ok(true) => ExitCode::SUCCESS,
        _ => ExitCode::FAILURE,
    }
}

async fn api(path: &str, namespace: &str, bnet: &BnetArgs) -> anyhow::Result<()> {
    let Some(config) = bnet.config()? else {
        anyhow::bail!(
            "set SQUADRON_BNET_CLIENT_ID and SQUADRON_BNET_CLIENT_SECRET (or put them in .env)"
        );
    };
    let (status, body) = armory::Client::new(config)?.raw(path, namespace).await?;
    println!("{status}");
    match serde_json::from_str::<serde_json::Value>(&body) {
        Ok(json) => println!("{}", serde_json::to_string_pretty(&json)?),
        Err(_) => println!("{body}"),
    }
    Ok(())
}
