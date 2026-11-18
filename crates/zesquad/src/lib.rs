//! zesquad: friend squads for WoW: Forever. A squad is a link (a random UUID) with a name and
//! a list of characters, each with free-form tags; character details come from the Battle.net
//! Armory in the background.

pub mod armory;
mod assets;
pub mod auth;
pub mod cli;
pub mod db;
pub mod sync;
pub mod version;
pub mod web;

use std::future::Future;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context as _;
use axum::http::StatusCode;
use tokio::sync::Notify;
use tower_http::compression::CompressionLayer;
use tower_http::timeout::TimeoutLayer;
use tower_http::trace::TraceLayer;

/// Everything `start` needs, independent of how it was configured.
pub struct Settings {
    pub listen: SocketAddr,
    pub database: PathBuf,
    pub refresh: Duration,
    pub region: String,
    pub armory: Option<armory::Config>,
    /// The shared password; `None` leaves the app open.
    pub password: Option<String>,
}

/// A started server.
pub struct Running {
    /// The bound address (useful when listening on port 0).
    pub addr: SocketAddr,
    pub server: tokio::task::JoinHandle<anyhow::Result<()>>,
}

/// Opens the database, binds the listener and spawns the server and the Armory sync. The server
/// stops (finishing open requests) when `shutdown` completes; the sync stops with it.
///
/// # Errors
///
/// When the database can't be opened, the address can't be bound or the HTTP client fails to
/// build.
pub async fn start(
    settings: Settings,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> anyhow::Result<Running> {
    let db = db::open(&settings.database).await?;
    let wake = Arc::new(Notify::new());

    let syncer = if let Some(config) = settings.armory {
        let syncer = sync::Syncer {
            db: db.clone(),
            client: Arc::new(armory::Client::new(config)?),
            interval: settings.refresh,
            wake: wake.clone(),
        };
        Some(tokio::spawn(syncer.run()))
    } else {
        tracing::warn!("no Battle.net credentials: Armory lookups are off");
        None
    };

    let auth = match settings.password.as_deref().filter(|p| !p.is_empty()) {
        Some(password) => Some(Arc::new(auth::Auth::new(&db, password).await?)),
        None => None,
    };
    let state = web::AppState {
        db,
        wake,
        region: settings.region,
        armory_enabled: syncer.is_some(),
        armory_cache: Arc::default(),
        auth,
    };
    let app = web::router(state).layer((
        TraceLayer::new_for_http(),
        CompressionLayer::new(),
        TimeoutLayer::with_status_code(StatusCode::REQUEST_TIMEOUT, Duration::from_secs(30)),
    ));

    let listener = tokio::net::TcpListener::bind(settings.listen)
        .await
        .with_context(|| format!("listening on {}", settings.listen))?;
    let addr = listener.local_addr()?;
    let server = tokio::spawn(async move {
        let served = axum::serve(listener, app)
            .with_graceful_shutdown(shutdown)
            .await;
        if let Some(syncer) = syncer {
            syncer.abort();
        }
        served.context("serving HTTP")
    });
    Ok(Running { addr, server })
}

/// Resolves on SIGINT or SIGTERM.
pub async fn shutdown_signal() {
    let ctrl_c = async {
        if let Err(e) = tokio::signal::ctrl_c().await {
            tracing::error!("can't listen for Ctrl-C: {e}");
            std::future::pending::<()>().await;
        }
    };
    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut s) => {
                s.recv().await;
            }
            Err(e) => {
                tracing::error!("can't listen for SIGTERM: {e}");
                std::future::pending::<()>().await;
            }
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        () = ctrl_c => {}
        () = terminate => {}
    }
    tracing::info!("shutting down");
}
