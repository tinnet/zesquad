//! Slowing down repeated logins and changes, per client (`zesquad_core::limit`).
//!
//! The counters live in memory, so a restart forgets them. Behind a reverse proxy every request
//! comes from the proxy's address; [`TrustProxy`] says which header the proxy puts the client's
//! address in. Without a proxy, or with the wrong header, clients could pick their own address,
//! so it's off unless asked for.

use std::net::{IpAddr, SocketAddr};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use axum::extract::{ConnectInfo, Request, State};
use axum::http::{HeaderMap, Method};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use zesquad_core::limit::{Counter, Rule, client_key, cloudflare_client, forwarded_for_client};

use crate::web::{AppError, AppState};

/// How many of each kind of request one client may make.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    /// Starting a login, with Battle.net or the shared password.
    pub login: Rule,
    /// Starting a squad.
    pub new_squad: Rule,
    /// Every other form: characters, tags, joining, settings.
    pub change: Rule,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            login: Rule {
                max: 10,
                window: 60,
            },
            new_squad: Rule {
                max: 10,
                window: 3600,
            },
            change: Rule {
                max: 60,
                window: 60,
            },
        }
    }
}

/// Where the client's address comes from.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, clap::ValueEnum)]
pub enum TrustProxy {
    /// The connection's own address: no proxy in front.
    #[default]
    None,
    /// Cloudflare's `CF-Connecting-IP` (a Cloudflare Tunnel or proxied DNS record).
    Cloudflare,
    /// The last `X-Forwarded-For` entry, which the proxy in front adds (Caddy, nginx and the like).
    ForwardedFor,
}

/// What a request counts towards.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Login,
    NewSquad,
    Change,
}

impl Kind {
    /// Pages and the roster's polling aren't limited; logging out never is.
    fn of(method: &Method, path: &str) -> Option<Self> {
        match (method, path) {
            (&Method::GET, "/auth/login") | (&Method::POST, "/login") => Some(Self::Login),
            (&Method::POST, "/squads") => Some(Self::NewSquad),
            (&Method::POST, "/logout") => None,
            (&Method::POST, _) => Some(Self::Change),
            _ => None,
        }
    }
}

pub struct Limiter {
    trust_proxy: TrustProxy,
    /// Warned that proxy headers arrive but aren't trusted.
    warned: AtomicBool,
    login: Mutex<Counter<IpAddr>>,
    new_squad: Mutex<Counter<IpAddr>>,
    change: Mutex<Counter<IpAddr>>,
}

impl Limiter {
    #[must_use]
    pub fn new(limits: Limits, trust_proxy: TrustProxy) -> Self {
        Self {
            trust_proxy,
            warned: AtomicBool::new(false),
            login: Mutex::new(Counter::new(limits.login)),
            new_squad: Mutex::new(Counter::new(limits.new_squad)),
            change: Mutex::new(Counter::new(limits.change)),
        }
    }

    /// The client behind a request: the proxy's word for it when trusted, else the peer.
    fn client(&self, headers: &HeaderMap, peer: Option<IpAddr>) -> Option<IpAddr> {
        let forwarded = match self.trust_proxy {
            TrustProxy::None => {
                self.warn_if_proxied(headers);
                None
            }
            TrustProxy::Cloudflare => cloudflare_client(
                headers
                    .get("cf-connecting-ip")
                    .and_then(|v| v.to_str().ok()),
            ),
            TrustProxy::ForwardedFor => forwarded_for_client(
                headers
                    .get_all("x-forwarded-for")
                    .iter()
                    .filter_map(|v| v.to_str().ok()),
            ),
        };
        forwarded.or(peer).map(client_key)
    }

    /// Behind a proxy without `ZESQUAD_TRUST_PROXY`, everyone shares the proxy's limits; say so
    /// once.
    fn warn_if_proxied(&self, headers: &HeaderMap) {
        let proxied =
            headers.contains_key("cf-connecting-ip") || headers.contains_key("x-forwarded-for");
        if proxied && !self.warned.swap(true, Ordering::Relaxed) {
            tracing::warn!(
                "requests come through a proxy, but ZESQUAD_TRUST_PROXY isn't set: every client \
                 shares the proxy's rate limits. Set it to cloudflare or forwarded-for"
            );
        }
    }

    /// Counts a request; `Err` with the seconds to wait when it's over the limit.
    fn hit(&self, kind: Kind, client: IpAddr) -> Result<(), u64> {
        let counter = match kind {
            Kind::Login => &self.login,
            Kind::NewSquad => &self.new_squad,
            Kind::Change => &self.change,
        };
        let now = u64::try_from(crate::db::now()).unwrap_or_default();
        // A poisoned lock only means another request panicked mid-count; the counts are fine.
        let mut counter = counter
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        counter.hit(client, now).map_err(|r| r.0)
    }
}

/// Middleware: refuses a request over its client's limit with 429 and `Retry-After`.
pub async fn limit(State(state): State<AppState>, request: Request, next: Next) -> Response {
    if let Some(kind) = Kind::of(request.method(), request.uri().path()) {
        let peer = request
            .extensions()
            .get::<ConnectInfo<SocketAddr>>()
            .map(|c| c.0.ip());
        if let Some(client) = state.limiter.client(request.headers(), peer)
            && let Err(retry_after) = state.limiter.hit(kind, client)
        {
            tracing::info!(%client, ?kind, "over the rate limit");
            return AppError::TooManyRequests { retry_after }.into_response();
        }
    }
    next.run(request).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_count_towards_their_kind() {
        assert_eq!(Kind::of(&Method::GET, "/auth/login"), Some(Kind::Login));
        assert_eq!(Kind::of(&Method::POST, "/login"), Some(Kind::Login));
        assert_eq!(Kind::of(&Method::POST, "/squads"), Some(Kind::NewSquad));
        assert_eq!(
            Kind::of(&Method::POST, "/s/x/characters"),
            Some(Kind::Change)
        );
        assert_eq!(Kind::of(&Method::POST, "/join/x"), Some(Kind::Change));
        assert_eq!(Kind::of(&Method::POST, "/logout"), None);
        assert_eq!(Kind::of(&Method::GET, "/s/x"), None);
        assert_eq!(Kind::of(&Method::GET, "/auth/callback"), None);
    }

    #[test]
    fn only_the_trusted_header_is_believed() {
        let ip = |s: &str| Some(s.parse::<IpAddr>().unwrap());
        let mut headers = HeaderMap::new();
        headers.insert("cf-connecting-ip", "192.0.2.7".parse().unwrap());
        headers.append("x-forwarded-for", "203.0.113.5".parse().unwrap());
        headers.append("x-forwarded-for", "198.51.100.9".parse().unwrap());
        let peer = ip("127.0.0.1");
        let with = |trust| Limiter::new(Limits::default(), trust);
        assert_eq!(
            with(TrustProxy::Cloudflare).client(&headers, peer),
            ip("192.0.2.7")
        );
        // The proxy's own header line, not the one the client sent.
        assert_eq!(
            with(TrustProxy::ForwardedFor).client(&headers, peer),
            ip("198.51.100.9")
        );
        let none = with(TrustProxy::None);
        assert_eq!(none.client(&headers, peer), peer);
        assert!(none.warned.load(Ordering::Relaxed));
        assert_eq!(
            with(TrustProxy::Cloudflare).client(&HeaderMap::new(), peer),
            peer
        );
    }
}
