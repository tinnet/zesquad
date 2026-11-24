//! Black-box tests: the real app on a random port with a temporary database, talking to a fake
//! Battle.net in the same process, driven over HTTP the way a browser (or htmx) would.

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;

use axum::Router;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use reqwest::redirect::Policy;
use tempfile::TempDir;
use zesquad::Settings;
use zesquad_core::Region;
use zesquad_core::armory::Endpoint;

const CLIENT_ID: &str = "test-id";
const CLIENT_SECRET: &str = "test-secret";
const TOKEN: &str = "test-token";
/// The made-up character the fake Armory knows (see zesquad-core/fixtures).
const KNOWN: &str = "aldric-thornwood";

// ---- fake Battle.net ----

#[derive(Clone, Default)]
struct Fake {
    lookups: Arc<AtomicUsize>,
    /// Character lookups wait while a test holds this for writing.
    gate: Arc<tokio::sync::RwLock<()>>,
    /// Character lookups answer 503.
    down: Arc<AtomicBool>,
    /// Auction dumps asked for, as `region/house[/commodities]`, with ` if-modified-since`
    /// for conditional requests.
    auction_requests: Arc<std::sync::Mutex<Vec<String>>>,
    /// The fake bucket: what was uploaded, by path (bucket and key).
    archived: Arc<std::sync::Mutex<std::collections::BTreeMap<String, Vec<u8>>>>,
    /// Uploads answer 503.
    archive_down: Arc<AtomicBool>,
    /// The level the known character's summary says; 0 keeps the fixture's.
    level: Arc<AtomicU64>,
    /// Messages posted to the fake Discord: the webhook path (with query) and the JSON body.
    discord: Arc<std::sync::Mutex<Vec<(String, serde_json::Value)>>>,
    /// Guild and roster requests.
    guild_lookups: Arc<AtomicUsize>,
}

/// A webhook id the fake Discord says is gone (404), like a deleted webhook.
const DELETED_WEBHOOK: &str = "404404404404404404";

/// Discord's webhook endpoint: takes JSON messages, except on the deleted webhook.
async fn fake_discord(
    State(fake): State<Fake>,
    Path((id, token)): Path<(String, String)>,
    axum::extract::RawQuery(query): axum::extract::RawQuery,
    axum::Json(body): axum::Json<serde_json::Value>,
) -> Response {
    if id == DELETED_WEBHOOK {
        return (
            StatusCode::NOT_FOUND,
            r#"{"message":"Unknown Webhook","code":10015}"#,
        )
            .into_response();
    }
    let path = format!("/api/webhooks/{id}/{token}?{}", query.unwrap_or_default());
    fake.discord.lock().unwrap().push((path, body));
    axum::Json(serde_json::json!({ "id": "1" })).into_response()
}

impl Fake {
    /// The text of every message posted to Discord so far.
    fn discord_messages(&self) -> Vec<String> {
        self.discord
            .lock()
            .unwrap()
            .iter()
            .map(|(_, body)| body["content"].as_str().unwrap_or_default().to_owned())
            .collect()
    }
}

const ARCHIVE_KEY_ID: &str = "test-archive-key";

/// A bucket that takes presigned PUTs: the signature itself is rusty-s3's; here, that the
/// request carries one for our key and region, and no secret.
async fn fake_archive(
    State(fake): State<Fake>,
    Path(path): Path<String>,
    Query(q): Query<std::collections::HashMap<String, String>>,
    body: axum::body::Bytes,
) -> Response {
    let param = |name: &str| q.get(name).map_or("", String::as_str);
    let signed = param("X-Amz-Algorithm") == "AWS4-HMAC-SHA256"
        && param("X-Amz-Credential").starts_with(&format!("{ARCHIVE_KEY_ID}/"))
        && param("X-Amz-Credential").ends_with("/auto/s3/aws4_request")
        && param("X-Amz-Signature").len() == 64
        && !q.values().any(|v| v.contains("test-archive-secret"));
    if !signed {
        return StatusCode::FORBIDDEN.into_response();
    }
    if fake.archive_down.load(Ordering::SeqCst) {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    fake.archived.lock().unwrap().insert(path, body.to_vec());
    StatusCode::OK.into_response()
}

/// Archives into `dir`, uploading to the fake's bucket under a prefix.
fn archive_config(fake_url: &str, dir: &std::path::Path) -> zesquad::archive::Config {
    zesquad::archive::Config {
        dir: dir.to_owned(),
        bucket: Some(zesquad::archive::BucketConfig {
            url: format!("{fake_url}/bucket/zesquad/").parse().unwrap(),
            region: "auto".into(),
            access_key_id: ARCHIVE_KEY_ID.into(),
            secret_access_key: "test-archive-secret".into(),
        }),
    }
}

/// A dump as archived: zstd-compressed JSON.
fn unzstd(bytes: &[u8]) -> String {
    String::from_utf8(zstd::decode_all(bytes).unwrap()).unwrap()
}

fn fixture(name: &str) -> String {
    std::fs::read_to_string(format!(
        "{}/../zesquad-core/fixtures/{name}.json",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap()
}

/// When the fake's auction dumps last changed.
const DUMPS_MODIFIED: &str = "Fri, 20 Nov 2026 20:23:48 GMT";

/// A player's token from the fake login, and the code it's swapped for.
const USER_TOKEN: &str = "test-user-token";
const GOOD_CODE: &str = "good-code";
/// Another player, a friend, logs in with this code.
const FRIEND_CODE: &str = "friend-code";
const FRIEND_TOKEN: &str = "test-friend-token";
/// And a third, a stranger.
const STRANGER_CODE: &str = "stranger-code";
const STRANGER_TOKEN: &str = "test-stranger-token";
/// Where the test app says it lives; Battle.net must be told the same redirect.
const PUBLIC_URL: &str = "https://zesquad.test";

/// The fake's token endpoint: the app's own token, or (with `grant_type=authorization_code`)
/// a player's, for the one good code and the app's registered redirect.
async fn fake_token(
    headers: HeaderMap,
    axum::Form(form): axum::Form<std::collections::HashMap<String, String>>,
) -> Response {
    // base64("test-id:test-secret")
    let expected = "Basic dGVzdC1pZDp0ZXN0LXNlY3JldA==";
    if headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        != Some(expected)
    {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    if form.get("grant_type").map(String::as_str) == Some("authorization_code") {
        let redirect = format!("{PUBLIC_URL}/auth/callback");
        let token = match form.get("code").map(String::as_str) {
            Some(GOOD_CODE) => USER_TOKEN,
            Some(FRIEND_CODE) => FRIEND_TOKEN,
            Some(STRANGER_CODE) => STRANGER_TOKEN,
            _ => return (StatusCode::BAD_REQUEST, r#"{"error":"invalid_grant"}"#).into_response(),
        };
        if form.get("redirect_uri") != Some(&redirect) {
            return (StatusCode::BAD_REQUEST, r#"{"error":"invalid_grant"}"#).into_response();
        }
        return axum::Json(serde_json::json!({
            "access_token": token, "token_type": "bearer", "expires_in": 86399,
            "scope": "openid"
        }))
        .into_response();
    }
    axum::Json(serde_json::json!({
        "access_token": TOKEN, "token_type": "bearer", "expires_in": 86399
    }))
    .into_response()
}

/// Who a player token belongs to: made-up accounts.
async fn fake_userinfo(headers: HeaderMap) -> Response {
    let bearer = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));
    let (id, battletag) = match bearer {
        Some(USER_TOKEN) => (424_242, "Testhero#1234"),
        Some(FRIEND_TOKEN) => (515_151, "Friendly#5678"),
        Some(STRANGER_TOKEN) => (616_161, "Stranger#9012"),
        _ => return StatusCode::UNAUTHORIZED.into_response(),
    };
    axum::Json(serde_json::json!({
        "sub": id.to_string(), "id": id, "battletag": battletag
    }))
    .into_response()
}

fn fake_character(
    fake: &Fake,
    headers: &HeaderMap,
    q: &std::collections::HashMap<String, String>,
    segments: &[&str],
) -> Response {
    fake.lookups.fetch_add(1, Ordering::SeqCst);
    let authorised = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        == Some(&format!("Bearer {TOKEN}"));
    if !authorised {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    if fake.down.load(Ordering::SeqCst) {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    // The fake answers for every region; the known character is in EU only.
    let namespace = q.get("namespace").map_or("", String::as_str);
    let in_eu = match namespace {
        "profile-forever-eu" => true,
        "profile-forever-us" | "profile-forever-tw" => false,
        _ => return StatusCode::FORBIDDEN.into_response(),
    };
    let not_found = (
        StatusCode::NOT_FOUND,
        r#"{"code":404,"type":"BLZWEBAPI00000404","detail":"Not Found"}"#,
    )
        .into_response();
    let Some((&key, path)) = segments.split_first() else {
        return not_found;
    };
    // No hunter pets: the fixture character is a rogue.
    let Some(endpoint) = Endpoint::ALL
        .into_iter()
        .find(|e| e.path() == path && *e != Endpoint::HunterPets)
    else {
        return not_found;
    };
    if key != KNOWN || !in_eu {
        return not_found;
    }
    let path = format!(
        "{}/../zesquad-core/fixtures/{}.json",
        env!("CARGO_MANIFEST_DIR"),
        endpoint.as_str()
    );
    let mut body = std::fs::read_to_string(path).unwrap();
    let level = fake.level.load(Ordering::SeqCst);
    if endpoint == Endpoint::Summary && level > 0 {
        let mut summary: serde_json::Value = serde_json::from_str(&body).unwrap();
        summary["level"] = level.into();
        body = summary.to_string();
    }
    ([(header::CONTENT_TYPE, "application/json")], body).into_response()
}

/// House 110 has the fixture auctions in every region and other houses are empty, except US
/// house 111, which is down.
async fn fake_auctions(
    State(fake): State<Fake>,
    headers: HeaderMap,
    Query(q): Query<std::collections::HashMap<String, String>>,
    Path((ruleset, rest)): Path<(u32, String)>,
) -> Response {
    let authorised = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        == Some(&format!("Bearer {TOKEN}"));
    if !authorised {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let region = match q.get("namespace").map(String::as_str) {
        Some("dynamic-forever-eu") => "eu",
        Some("dynamic-forever-us") => "us",
        Some("dynamic-forever-tw") => "tw",
        _ => return StatusCode::FORBIDDEN.into_response(),
    };
    if !(1..=3).contains(&ruleset) && ruleset != 15 {
        return StatusCode::NOT_FOUND.into_response();
    }
    let since = headers
        .get(header::IF_MODIFIED_SINCE)
        .and_then(|v| v.to_str().ok());
    fake.auction_requests.lock().unwrap().push(match since {
        Some(_) => format!("{region}/{rest} if-modified-since"),
        None => format!("{region}/{rest}"),
    });
    let (house, commodities) = match rest.split_once('/') {
        Some((house, "commodities")) => (house, true),
        None => (rest.as_str(), false),
        Some(_) => return StatusCode::NOT_FOUND.into_response(),
    };
    if (region, house) == ("us", "111") {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    // EU house 113's commodities changed later than the rest.
    let modified = if (region, house, commodities) == ("eu", "113", true) {
        "Fri, 20 Nov 2026 21:23:48 GMT"
    } else {
        DUMPS_MODIFIED
    };
    if since == Some(DUMPS_MODIFIED) && modified == DUMPS_MODIFIED {
        return StatusCode::NOT_MODIFIED.into_response();
    }
    let body = if house == "110" {
        let name = if commodities {
            "commodities"
        } else {
            "auctions"
        };
        std::fs::read_to_string(format!(
            "{}/../zesquad-core/fixtures/{name}.json",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap()
    } else {
        r#"{"auctions":[]}"#.to_owned()
    };
    (
        [
            (header::CONTENT_TYPE, "application/json"),
            (header::LAST_MODIFIED, modified),
        ],
        body,
    )
        .into_response()
}

async fn start_fake() -> (String, Fake) {
    let fake = Fake::default();
    let app = Router::new()
        .route("/token", post(fake_token))
        .route("/userinfo", get(fake_userinfo))
        .route("/profile/wow/character/{*rest}", get(fake_character_path))
        .route("/data/wow/guild/{*rest}", get(fake_guild))
        .route(
            "/data/wow/ruleset/{ruleset}/auctions/{*rest}",
            get(fake_auctions),
        )
        .route("/bucket/{*path}", put(fake_archive))
        .route("/api/webhooks/{id}/{token}", post(fake_discord))
        .with_state(fake.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await });
    (format!("http://{addr}"), fake)
}

/// Splits the wildcard into segments for `fake_character`.
async fn fake_character_path(
    State(fake): State<Fake>,
    headers: HeaderMap,
    Query(q): Query<std::collections::HashMap<String, String>>,
    Path(rest): Path<String>,
) -> Response {
    let _open = fake.gate.read().await;
    let segments: Vec<&str> = rest.split('/').collect();
    fake_character(&fake, &headers, &q, &segments)
}

/// The made-up guild the fake Armory knows, in EU only (see zesquad-core/fixtures).
const GUILD: &str = "order-of-the-lantern";

/// `/data/wow/guild/{slug}[/roster]`: the fixture guild and its 61 members.
async fn fake_guild(
    State(fake): State<Fake>,
    headers: HeaderMap,
    Query(q): Query<std::collections::HashMap<String, String>>,
    Path(rest): Path<String>,
) -> Response {
    fake.guild_lookups.fetch_add(1, Ordering::SeqCst);
    if headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        != Some(&format!("Bearer {TOKEN}"))
    {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let in_eu = q.get("namespace").map(String::as_str) == Some("profile-forever-eu");
    let fixture_name = match rest.split_once('/') {
        None if rest == GUILD => "guild",
        Some((GUILD, "roster")) => "guild-roster",
        _ => "",
    };
    if fixture_name.is_empty() || !in_eu {
        return (
            StatusCode::NOT_FOUND,
            r#"{"code":404,"type":"BLZWEBAPI00000404","detail":"Not Found"}"#,
        )
            .into_response();
    }
    (
        [(header::CONTENT_TYPE, "application/json")],
        fixture(fixture_name),
    )
        .into_response()
}

// ---- the app ----

struct App {
    base: String,
    http: reqwest::Client,
    fake: Fake,
    dir: TempDir,
}

async fn app(with_armory: bool) -> App {
    app_with(with_armory, None).await
}

async fn app_with(with_armory: bool, password: Option<&str>) -> App {
    app_with_auctions(with_armory, password, false).await
}

async fn app_with_auctions(with_armory: bool, password: Option<&str>, auctions: bool) -> App {
    app_full(with_armory, password, auctions, false).await
}

/// With logging in through the fake Battle.net.
async fn app_with_login(password: Option<&str>) -> App {
    app_full(true, password, false, true).await
}

async fn app_full(with_armory: bool, password: Option<&str>, auctions: bool, login: bool) -> App {
    app_tweaked(with_armory, password, auctions, login, |_| {}).await
}

/// `app_full`, with the settings changed by `tweak` before starting.
async fn app_tweaked(
    with_armory: bool,
    password: Option<&str>,
    auctions: bool,
    login: bool,
    tweak: impl FnOnce(&mut Settings),
) -> App {
    let dir = tempfile::tempdir().unwrap();
    let (fake_url, fake) = start_fake().await;
    let armory = with_armory.then(|| {
        zesquad::armory::Config::new(
            CLIENT_ID.into(),
            CLIENT_SECRET.into(),
            Some(&fake_url),
            &format!("{fake_url}/token"),
        )
        .unwrap()
    });
    let mut settings = Settings {
        listen: SocketAddr::from(([127, 0, 0, 1], 0)),
        database: dir.path().join("zesquad.db"),
        refresh: Duration::from_secs(3600),
        default_region: Region::Eu,
        armory,
        password: password.map(str::to_owned),
        auctions,
        login: login.then(|| zesquad::login::Config {
            client_id: CLIENT_ID.into(),
            client_secret: CLIENT_SECRET.into(),
            oauth_url: reqwest::Url::parse(&format!("{fake_url}/")).unwrap(),
            token_url: reqwest::Url::parse(&format!("{fake_url}/token")).unwrap(),
            public_url: reqwest::Url::parse(PUBLIC_URL).unwrap(),
        }),
        archive: auctions.then(|| archive_config(&fake_url, &dir.path().join("archive"))),
        discord: zesquad::discord::Config {
            api_url: reqwest::Url::parse(&fake_url).unwrap(),
            public_url: Some(reqwest::Url::parse(PUBLIC_URL).unwrap()),
        },
        limits: zesquad::limits::Limits::default(),
        trust_proxy: zesquad::limits::TrustProxy::None,
    };
    tweak(&mut settings);
    let running = zesquad::start(settings, std::future::pending())
        .await
        .unwrap();
    App {
        base: format!("http://{}", running.addr),
        http: reqwest::Client::builder()
            .redirect(Policy::none())
            .build()
            .unwrap(),
        fake,
        dir,
    }
}

impl App {
    async fn get(&self, path: &str) -> (StatusCode, String) {
        let r = self
            .http
            .get(format!("{}{path}", self.base))
            .send()
            .await
            .unwrap();
        (r.status(), r.text().await.unwrap())
    }

    async fn post(&self, path: &str, form: &[(&str, &str)]) -> reqwest::Response {
        self.http
            .post(format!("{}{path}", self.base))
            .form(form)
            .send()
            .await
            .unwrap()
    }

    /// Posts like htmx does and returns the swapped-in roster.
    async fn htmx(&self, path: &str, form: &[(&str, &str)]) -> (StatusCode, String) {
        let r = self
            .http
            .post(format!("{}{path}", self.base))
            .header("HX-Request", "true")
            .form(form)
            .send()
            .await
            .unwrap();
        (r.status(), r.text().await.unwrap())
    }

    /// Creates a squad and returns its path, `/s/<id>`.
    async fn squad(&self, name: &str) -> String {
        let r = self.post("/squads", &[("name", name)]).await;
        assert_eq!(r.status(), StatusCode::SEE_OTHER);
        r.headers()[header::LOCATION].to_str().unwrap().to_owned()
    }

    async fn add(&self, squad: &str, character: &str, tags: &str) -> reqwest::Response {
        self.post(
            &format!("{squad}/characters"),
            &[("character", character), ("tags", tags)],
        )
        .await
    }

    /// Polls the squad page until it contains `text` (the Armory sync runs in the background).
    async fn wait_for(&self, path: &str, text: &str) -> String {
        for _ in 0..100 {
            let (_, body) = self.get(path).await;
            if body.contains(text) {
                return body;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("{path} never showed {text:?}");
    }
}

/// The id of the n-th member on a page, from its `id="member-<id>"`.
fn member_id(page: &str, n: usize) -> String {
    page.split("id=\"member-")
        .nth(n + 1)
        .and_then(|s| s.split('"').next())
        .unwrap()
        .to_owned()
}

// ---- tests ----

#[tokio::test]
async fn health_version_and_static_files() {
    let app = app(false).await;
    assert_eq!(app.get("/healthz").await, (StatusCode::OK, "ok".into()));
    let (status, body) = app.get("/version").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("\"version\""), "{body}");

    let (_, page) = app.get("/").await;
    let css = page
        .split("href=\"")
        .find_map(|s| s.split('"').next().filter(|h| h.ends_with("style.css")))
        .unwrap();
    let r = app
        .http
        .get(format!("{}{css}", app.base))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    assert_eq!(r.headers()[header::CONTENT_TYPE], "text/css");
    assert_eq!(app.get("/static/x/nope.js").await.0, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn creating_a_squad_leads_to_its_page() {
    let app = app(false).await;
    let squad = app.squad("  Thursday   raiders ").await;
    let id = squad.strip_prefix("/s/").unwrap();
    assert_eq!(id.len(), 26, "{id}");
    assert!(
        id.bytes()
            .all(|c| c.is_ascii_digit() || c.is_ascii_lowercase() && !b"ilou".contains(&c)),
        "{id}"
    );
    let (status, page) = app.get(&squad).await;
    assert_eq!(status, StatusCode::OK);
    assert!(page.contains(">Thursday raiders</h1>"), "{page}");
    assert!(page.contains("No characters yet"));
}

#[tokio::test]
async fn squads_with_an_old_uuid_link_still_open() {
    let app = app(false).await;
    let old = "3f2b9c1e-8a4d-4e7f-9b21-6c0d5a7e8f13";
    let db = zesquad::db::open(&app.dir.path().join("zesquad.db"), Region::Eu)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO squads (id, name, region, created_at) VALUES (?, 'Old raiders', 'eu', 0)",
    )
    .bind(old)
    .execute(&db)
    .await
    .unwrap();
    let (status, page) = app.get(&format!("/s/{old}")).await;
    assert_eq!(status, StatusCode::OK);
    assert!(page.contains(">Old raiders</h1>"), "{page}");
}

#[tokio::test]
async fn a_squad_needs_a_name() {
    let app = app(false).await;
    let r = app.post("/squads", &[("name", "   ")]).await;
    assert_eq!(r.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert!(r.text().await.unwrap().contains("give the squad a name"));
}

#[tokio::test]
async fn unknown_squads_and_pages_are_404() {
    let app = app(false).await;
    let (status, body) = app.get("/s/00000000-0000-0000-0000-000000000000").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(body.contains("Not found"));
    assert_eq!(app.get("/nope").await.0, StatusCode::NOT_FOUND);
    let r = app
        .add(
            "/s/00000000-0000-0000-0000-000000000000",
            "Aldric-Thornwood",
            "",
        )
        .await;
    assert_eq!(r.status(), StatusCode::NOT_FOUND);
}

/// The `zesquad_recent` cookie a response sets, as `name=value`.
fn recent_cookie(r: &reqwest::Response) -> String {
    r.headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find(|v| v.starts_with("zesquad_recent="))
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned()
}

impl App {
    /// GETs `path` with `cookie`, returning the response.
    async fn get_with(&self, path: &str, cookie: &str) -> reqwest::Response {
        self.http
            .get(format!("{}{path}", self.base))
            .header(header::COOKIE, cookie)
            .send()
            .await
            .unwrap()
    }
}

#[tokio::test]
async fn the_front_page_lists_squads_you_opened() {
    let app = app(false).await;
    let (_, page) = app.get("/").await;
    assert!(!page.contains("Your squads"), "{page}");

    let raiders = app.squad("Raiders").await;
    let alts = app.squad("Alt army").await;
    app.add(&raiders, "Aldric-Thornwood", "").await;

    // Each squad page remembers itself, newest first.
    let r = app.get_with(&raiders, "").await;
    let set = r.headers()[header::SET_COOKIE].to_str().unwrap().to_owned();
    assert!(
        set.contains("HttpOnly") && set.contains("SameSite=Lax") && !set.contains("Secure"),
        "{set}"
    );
    let cookie = recent_cookie(&r);
    let cookie = recent_cookie(&app.get_with(&alts, &cookie).await);
    let page = app.get_with("/", &cookie).await.text().await.unwrap();
    let (first, second) = (
        page.find("Alt army").unwrap(),
        page.find("Raiders").unwrap(),
    );
    assert!(first < second, "{page}");
    assert!(
        page.contains("1 character<") && page.contains("0 characters"),
        "{page}"
    );
    assert!(page.contains(&format!("href=\"{raiders}\"")), "{page}");

    // Opening one again moves it to the top.
    let cookie = recent_cookie(&app.get_with(&raiders, &cookie).await);
    let page = app.get_with("/", &cookie).await.text().await.unwrap();
    assert!(page.find("Raiders").unwrap() < page.find("Alt army").unwrap());

    // Forgetting takes it off the list and leaves the squad alone.
    let alts_id = alts.strip_prefix("/s/").unwrap();
    let r = app
        .http
        .post(format!("{}/recent/{alts_id}/forget", app.base))
        .header(header::COOKIE, &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::SEE_OTHER);
    assert_eq!(r.headers()[header::LOCATION], "/");
    let cookie = recent_cookie(&r);
    let page = app.get_with("/", &cookie).await.text().await.unwrap();
    assert!(
        page.contains("Raiders") && !page.contains("Alt army"),
        "{page}"
    );
    assert_eq!(app.get(&alts).await.0, StatusCode::OK);

    // Forgetting the last one clears the cookie.
    let raiders_id = raiders.strip_prefix("/s/").unwrap();
    let r = app
        .http
        .post(format!("{}/recent/{raiders_id}/forget", app.base))
        .header(header::COOKIE, &cookie)
        .send()
        .await
        .unwrap();
    assert!(
        r.headers()[header::SET_COOKIE]
            .to_str()
            .unwrap()
            .contains("Max-Age=0")
    );
}

#[tokio::test]
async fn the_front_page_shows_newcomers_an_example_squad() {
    let app = app(false).await;
    let (_, page) = app.get("/").await;
    assert!(page.contains("How it works"), "{page}");
    let example = page.split("What a squad looks like").nth(1).unwrap();
    assert_eq!(example.matches("class=\"member ").count(), 3, "{example}");
    assert!(example.contains("Level 60"), "{example}");
    assert!(
        example.contains("3 characters · 2 at level 60 · 3 Alliance"),
        "{example}"
    );
    // Class tags from the made-up Armory data, typed tags, and nothing to click that changes
    // anything or leads to a squad that doesn't exist.
    assert!(
        example.contains(">Priest<") && example.contains(">raid<"),
        "{example}"
    );
    for absent in ["<form", "/s/example", "Remove", "+ tag"] {
        assert!(!example.contains(absent), "{absent} in {example}");
    }

    // Someone with squads gets their list instead.
    let squad = app.squad("Raiders").await;
    let r = app.get_with(&squad, "").await;
    let page = app
        .get_with("/", &recent_cookie(&r))
        .await
        .text()
        .await
        .unwrap();
    assert!(
        page.contains("Your squads") && !page.contains("How it works"),
        "{page}"
    );
}

#[tokio::test]
async fn squads_that_are_gone_or_made_up_drop_off_the_list() {
    let app = app(false).await;
    let raiders = app.squad("Raiders").await;
    let raiders_id = raiders.strip_prefix("/s/").unwrap();
    let gone = "00000000000000000000000001";
    let cookie = format!("zesquad_recent={gone}.{raiders_id}.<b>nope</b>");
    let r = app.get_with("/", &cookie).await;
    assert_eq!(r.status(), StatusCode::OK);
    // The cookie is written back without the squad that doesn't exist.
    assert_eq!(recent_cookie(&r), format!("zesquad_recent={raiders_id}"));
    let page = r.text().await.unwrap();
    assert!(page.contains("Raiders") && !page.contains("nope"), "{page}");

    // A 404 isn't remembered.
    let r = app.get_with(&format!("/s/{gone}"), "").await;
    assert_eq!(r.status(), StatusCode::NOT_FOUND);
    assert!(r.headers().get(header::SET_COOKIE).is_none());
}

#[tokio::test]
async fn the_recent_cookie_is_secure_behind_https() {
    let app = app(false).await;
    let squad = app.squad("Raiders").await;
    let r = app
        .http
        .get(format!("{}{squad}", app.base))
        .header("x-forwarded-proto", "https")
        .send()
        .await
        .unwrap();
    assert!(
        r.headers()[header::SET_COOKIE]
            .to_str()
            .unwrap()
            .contains("; Secure")
    );
}

#[tokio::test]
async fn a_squad_page_shows_its_full_link_to_copy() {
    let app = app(false).await;
    let squad = app.squad("Raiders").await;
    let (_, page) = app.get(&squad).await;
    let link = format!("{}{squad}", app.base);
    assert!(
        page.contains(&format!("value=\"{link}\" readonly")),
        "{page}"
    );
    assert!(page.contains("data-copy=\"squad-link\""), "{page}");
    let script = page
        .split("src=\"")
        .find_map(|s| s.split('"').next().filter(|h| h.ends_with("share.js")))
        .unwrap();
    assert_eq!(app.get(script).await.0, StatusCode::OK);

    // Behind a proxy, the link is the one the browser used.
    let page = app
        .http
        .get(format!("{}{squad}", app.base))
        .header("x-forwarded-host", "squads.example")
        .header("x-forwarded-proto", "https")
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(
        page.contains(&format!("value=\"https://squads.example{squad}\"")),
        "{page}"
    );

    // A host that isn't one leaves just the path.
    let page = app
        .http
        .get(format!("{}{squad}", app.base))
        .header("x-forwarded-host", "evil.example/\"><script>")
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(page.contains(&format!("value=\"{squad}\"")), "{page}");
}

#[tokio::test]
async fn link_previews_show_the_squad_and_pages_have_an_icon() {
    let app = app(false).await;
    let (_, front) = app.get("/").await;
    assert!(
        front.contains(r#"<meta property="og:title" content="zesquad">"#),
        "{front}"
    );
    let icon = front
        .split("href=\"")
        .find_map(|s| s.split('"').next().filter(|h| h.ends_with("favicon.svg")))
        .unwrap();
    let r = app
        .http
        .get(format!("{}{icon}", app.base))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    assert_eq!(r.headers()[header::CONTENT_TYPE], "image/svg+xml");

    let squad = app.squad("Thursday <raiders>").await;
    app.add(&squad, "Aldric-Thornwood", "raid").await;
    app.add(&squad, "Brena-Ashvale", "").await;
    // The count is the whole squad's, whatever the filter.
    let (_, page) = app.get(&format!("{squad}?tag=raid")).await;
    assert!(
        page.contains(r#"<meta property="og:title" content="Thursday &#60;raiders&#62;">"#),
        "{page}"
    );
    assert!(
        page.contains(
            r#"<meta property="og:description" content="2 characters in Europe on zesquad"#
        ),
        "{page}"
    );
    assert_eq!(page.matches("property=\"og:title\"").count(), 1, "{page}");
}

#[tokio::test]
async fn renaming_a_squad() {
    let app = app(false).await;
    let squad = app.squad("Old name").await;
    let (_, page) = app.get(&squad).await;
    assert!(
        page.contains(&format!("href=\"{squad}/settings\"")),
        "{page}"
    );
    let r = app
        .post(&format!("{squad}/name"), &[("name", "New name")])
        .await;
    assert_eq!(r.status(), StatusCode::SEE_OTHER);
    assert_eq!(r.headers()[header::LOCATION], format!("{squad}/settings"));
    assert!(
        app.get(&format!("{squad}/settings"))
            .await
            .1
            .contains("value=\"New name\"")
    );
    assert!(app.get(&squad).await.1.contains(">New name</h1>"));
}

#[tokio::test]
async fn added_characters_get_their_armory_details() {
    let app = app(true).await;
    let squad = app.squad("Raiders").await;
    let r = app.add(&squad, " aldric-THORNWOOD ", "Raid, alts").await;
    assert_eq!(r.status(), StatusCode::SEE_OTHER);

    let page = app.wait_for(&squad, "Night Elf").await;
    for shown in [
        "Aldric <span class=\"surname\">Thornwood</span>",
        "Level 26",
        "Rogue",
        "<span class=\"ruleset\" title=\"Ruleset\">Normal</span>",
        "&lt;Order of the Lantern&gt;</a> · item level 14 · Combat 17 / Assassination 5",
        "Red Defias Mask",
        "Skinning 158/225",
        "https://render.example/forever-eu/character/1-avatar.jpg",
        "class-rogue",
        ">Raid<",
        ">alts<",
    ] {
        assert!(page.contains(shown), "missing {shown:?} in {page}");
    }
    assert!(!page.contains("Pets"), "a rogue has no pets section");
}

#[tokio::test]
async fn characters_the_armory_doesnt_know_are_marked() {
    let app = app(true).await;
    let squad = app.squad("Raiders").await;
    app.add(&squad, "Nobody-Atall", "").await;
    app.wait_for(&squad, "Not on the Armory").await;
}

#[tokio::test]
async fn a_squad_looks_its_characters_up_in_its_region() {
    let app = app(true).await;
    let (status, index) = app.get("/").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        index.contains(r#"<option value="eu" selected>Europe</option>"#),
        "{index}"
    );
    assert!(
        index.contains(r#"<option value="us">Americas</option>"#),
        "{index}"
    );

    let r = app
        .post("/squads", &[("name", "Raiders"), ("region", "us")])
        .await;
    assert_eq!(r.status(), StatusCode::SEE_OTHER);
    let squad = r.headers()[header::LOCATION].to_str().unwrap().to_owned();
    let (_, page) = app.get(&squad).await;
    assert!(page.contains("Characters from Americas"), "{page}");
    // The fake knows this character in EU only.
    app.add(&squad, "Aldric-Thornwood", "").await;
    app.wait_for(&squad, "Not on the Armory").await;

    let eu = app.squad("Raiders").await;
    app.add(&eu, "Aldric-Thornwood", "").await;
    app.wait_for(&eu, "Level ").await;
}

#[tokio::test]
async fn a_squad_needs_a_known_region() {
    let app = app(false).await;
    let r = app
        .post("/squads", &[("name", "Raiders"), ("region", "kr")])
        .await;
    assert_eq!(r.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let page = r.text().await.unwrap();
    assert!(page.contains("unknown region"), "{page}");
    assert!(page.contains(r#"value="Raiders""#), "{page}");
}

#[tokio::test]
async fn squads_from_before_regions_get_their_characters_region_or_the_default() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("zesquad.db");
    // A database as it was before migration 0004 gave squads a region.
    let old = sqlx::sqlite::SqlitePoolOptions::new()
        .connect_with(
            sqlx::sqlite::SqliteConnectOptions::new()
                .filename(&path)
                .create_if_missing(true),
        )
        .await
        .unwrap();
    let mut before = sqlx::migrate!("./migrations");
    before.migrations = before
        .migrations
        .iter()
        .filter(|m| m.version < 4)
        .cloned()
        .collect::<Vec<_>>()
        .into();
    before.run(&old).await.unwrap();
    sqlx::query(
        "INSERT INTO squads (id, name, created_at) VALUES ('empty', 'Empty', 0), ('full', 'Full', 0);
         INSERT INTO characters (squad_id, region, name, surname, char_key, added_at)
         VALUES ('full', 'us', 'Bryn', 'Ashvale', 'bryn-ashvale', 0);",
    )
    .execute(&old)
    .await
    .unwrap();
    old.close().await;

    let db = zesquad::db::open(&path, Region::Tw).await.unwrap();
    for (id, region) in [("empty", Region::Tw), ("full", Region::Us)] {
        let squad = zesquad::db::squad(&db, id).await.unwrap().unwrap();
        assert_eq!(squad.region, region, "{id}");
    }
}

#[tokio::test]
async fn refresh_fetches_again() {
    let app = app(true).await;
    let squad = app.squad("Raiders").await;
    app.add(&squad, "Aldric-Thornwood", "").await;
    let page = app.wait_for(&squad, "Night Elf").await;
    let before = app.fake.lookups.load(Ordering::SeqCst);
    let id = member_id(&page, 0);
    let r = app
        .post(&format!("{squad}/characters/{id}/refresh"), &[])
        .await;
    assert_eq!(r.status(), StatusCode::SEE_OTHER);
    for _ in 0..100 {
        if app.fake.lookups.load(Ordering::SeqCst) >= before + 5 {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("no new lookups after refresh");
}

const WEBHOOK_ID: &str = "123456789012345678";
const WEBHOOK_TOKEN: &str = "fake-Token_abc";

impl App {
    /// Waits for the fake Discord to have `n` messages and returns their text.
    async fn discord_messages(&self, n: usize) -> Vec<String> {
        for _ in 0..100 {
            let messages = self.fake.discord_messages();
            if messages.len() >= n {
                return messages;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!(
            "Discord got {:?}, not {n} messages",
            self.fake.discord_messages()
        );
    }
}

#[tokio::test]
async fn a_squad_posts_joins_and_level_ups_to_discord() {
    let app = app(true).await;
    let squad = app.squad("Thursday raiders").await;
    let id = squad.strip_prefix("/s/").unwrap();
    let settings = format!("{squad}/settings");
    // It's set up on the settings page, out of the roster's way.
    let (_, page) = app.get(&squad).await;
    assert!(!page.contains("Discord"), "{page}");
    let (_, page) = app.get(&settings).await;
    assert!(page.contains("Post to Discord"), "{page}");

    // Setting it up says hello, so whoever pasted the URL sees it work.
    let url = format!("https://discord.com/api/webhooks/{WEBHOOK_ID}/{WEBHOOK_TOKEN}");
    let r = app
        .post(&format!("{squad}/discord"), &[("webhook", url.as_str())])
        .await;
    assert_eq!(r.status(), StatusCode::SEE_OTHER);
    let hello = app.discord_messages(1).await;
    assert!(
        hello[0].contains(&format!("[Thursday raiders](<{PUBLIC_URL}/s/{id}>)")),
        "{hello:?}"
    );
    {
        let posts = app.fake.discord.lock().unwrap();
        let (path, body) = &posts[0];
        assert_eq!(
            path,
            &format!("/api/webhooks/{WEBHOOK_ID}/{WEBHOOK_TOKEN}?wait=true")
        );
        assert_eq!(body["allowed_mentions"]["parse"], serde_json::json!([]));
    }
    // The page says it's on but never shows the token.
    let (_, page) = app.get(&settings).await;
    assert!(
        page.contains("Posting to Discord") && page.contains("…5678"),
        "{page}"
    );
    assert!(!page.contains(WEBHOOK_TOKEN), "{page}");

    // A character joining, then going up a level on a later refresh.
    app.add(&squad, "Aldric-Thornwood", "").await;
    let page = app.wait_for(&squad, "Level 26").await;
    let messages = app.discord_messages(2).await;
    assert!(
        messages[1].starts_with("**Aldric Thornwood** joined [Thursday raiders]"),
        "{messages:?}"
    );
    // Each news message has a line of flavour under it, in Discord's small print.
    assert!(messages[1].contains(").\n-# "), "{messages:?}");
    app.fake.level.store(27, Ordering::SeqCst);
    let member = member_id(&page, 0);
    app.post(&format!("{squad}/characters/{member}/refresh"), &[])
        .await;
    let messages = app.discord_messages(3).await;
    assert!(
        messages[2].starts_with("**Aldric Thornwood** reached level 27 · [Thursday raiders]"),
        "{messages:?}"
    );
    assert!(messages[2].contains(">)\n-# "), "{messages:?}");
    // The same level again says nothing: wait for the refresh's lookups to be done.
    let lookups = app.fake.lookups.load(Ordering::SeqCst);
    app.post(&format!("{squad}/characters/{member}/refresh"), &[])
        .await;
    for _ in 0..100 {
        if app.fake.lookups.load(Ordering::SeqCst) >= lookups + 5 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(app.fake.discord_messages().len(), 3);

    // Stopping.
    let r = app.post(&format!("{squad}/discord/delete"), &[]).await;
    assert_eq!(r.status(), StatusCode::SEE_OTHER);
    assert!(app.get(&settings).await.1.contains("Post to Discord"));
    app.add(&squad, "Brena-Ashvale", "").await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(app.fake.discord_messages().len(), 3);
}

#[tokio::test]
async fn only_working_discord_webhooks_are_kept() {
    let app = app(false).await;
    let squad = app.squad("Raiders").await;
    for (url, says) in [
        (
            "https://evil.example/api/webhooks/1/x",
            "isn&#39;t a Discord webhook URL",
        ),
        ("not a url", "isn&#39;t a Discord webhook URL"),
        (
            &*format!("https://discord.com/api/webhooks/{DELETED_WEBHOOK}/{WEBHOOK_TOKEN}"),
            "Discord didn&#39;t take a message",
        ),
    ] {
        let r = app
            .post(&format!("{squad}/discord"), &[("webhook", url)])
            .await;
        assert_eq!(r.status(), StatusCode::UNPROCESSABLE_ENTITY, "{url}");
        let page = r.text().await.unwrap();
        assert!(page.contains(says), "{url}: {page}");
        assert!(page.contains("Post to Discord"), "{page}");
    }
    assert_eq!(app.fake.discord_messages(), Vec::<String>::new());
}

#[tokio::test]
async fn polling_waits_while_someone_is_typing() {
    let app = app(true).await;
    let squad = app.squad("Raiders").await;
    let _held = app.fake.gate.write().await;
    let (_, roster) = app
        .htmx(
            &format!("{squad}/characters"),
            &[("character", "Aldric-Thornwood"), ("tags", "")],
        )
        .await;
    assert!(roster.contains("Looking up on the Armory"), "{roster}");
    // The poll's filter skips a tick while an input is focused (a colour picker too) or a text
    // input is not empty. Every text input needs a placeholder for `:placeholder-shown` to
    // tell. An open colours panel doesn't stop it, or picking a colour would stop the polling.
    assert!(
        roster.contains(
            "hx-trigger=\"every 3s [!this.querySelector('input:not([type=hidden]):focus, \
             input:not([type=hidden]):not([type=color]):not(:placeholder-shown)')]\""
        ),
        "{roster}"
    );
    for input in roster.split("<input").skip(1) {
        let input = input.split('>').next().unwrap();
        assert!(
            input.contains("type=\"hidden\"")
                || input.contains("type=\"color\"")
                || input.contains("placeholder=\""),
            "text input without a placeholder: <input{input}>"
        );
    }
}

#[tokio::test]
async fn armory_details_stay_while_refreshing_and_after_a_failure() {
    let app = app(true).await;
    let squad = app.squad("Raiders").await;
    app.add(&squad, "Aldric-Thornwood", "").await;
    let page = app.wait_for(&squad, "Level 26").await;
    let refresh = format!("{squad}/characters/{}/refresh", member_id(&page, 0));

    let held = app.fake.gate.write().await;
    app.post(&refresh, &[]).await;
    let (_, page) = app.get(&squad).await;
    assert!(
        page.contains("Updating from the Armory") && page.contains("Level 26"),
        "{page}"
    );
    app.fake.down.store(true, Ordering::SeqCst);
    drop(held);

    let page = app.wait_for(&squad, "what it said before").await;
    assert!(page.contains("Couldn't reach the Armory"), "{page}");
    for shown in [
        "Level 26",
        "Rogue",
        "Order of the Lantern",
        "Red Defias Mask",
    ] {
        assert!(page.contains(shown), "missing {shown:?} in {page}");
    }
}

#[tokio::test]
async fn a_form_post_returns_to_the_same_filter() {
    let app = app(false).await;
    let squad = app.squad("Raiders").await;
    let r = app
        .post(
            &format!("{squad}/characters"),
            &[
                ("character", "Aldric-Thornwood"),
                ("tags", "Main tank"),
                ("filter", "Main TANK"),
            ],
        )
        .await;
    assert_eq!(r.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        r.headers()[header::LOCATION],
        format!("{squad}?tag=main%20tank")
    );
}

/// The members' names on a page, in order.
fn member_names(page: &str) -> Vec<&str> {
    page.split("<h2 class=\"name\">")
        .skip(1)
        .filter_map(|s| s.split(' ').next())
        .collect()
}

#[tokio::test]
async fn the_roster_sorts_and_sums_up() {
    let app = app(true).await;
    let squad = app.squad("Raiders").await;
    app.add(&squad, "Zora-Unknown", "raid").await;
    app.add(&squad, "Aldric-Thornwood", "raid").await;
    app.add(&squad, "Bryn-Unknown", "").await;
    app.wait_for(&squad, "Night Elf").await;
    app.wait_for(&squad, "Not on the Armory").await;

    let (_, page) = app.get(&squad).await;
    assert_eq!(member_names(&page), ["Zora", "Aldric", "Bryn"], "{page}");
    assert!(
        page.contains("3 characters · 1 at level 26 · 1 Alliance"),
        "{page}"
    );
    // The level sort puts characters with a level first, then by name.
    let (_, page) = app.get(&format!("{squad}?sort=level")).await;
    assert_eq!(member_names(&page), ["Aldric", "Bryn", "Zora"], "{page}");
    assert!(page.contains("aria-current=\"true\">level<"), "{page}");
    let (_, page) = app.get(&format!("{squad}?sort=name")).await;
    assert_eq!(member_names(&page), ["Aldric", "Bryn", "Zora"]);
    // An unknown sort is the default rather than an error.
    let (status, page) = app.get(&format!("{squad}?sort=nonsense")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(member_names(&page), ["Zora", "Aldric", "Bryn"]);

    // Filter and sort together: the summary counts what's shown, links keep both.
    let (_, page) = app.get(&format!("{squad}?tag=raid&sort=name")).await;
    assert_eq!(member_names(&page), ["Aldric", "Zora"]);
    assert!(page.contains("2 of 3 characters"), "{page}");
    assert!(
        page.contains(&format!("href=\"{squad}?sort=name\"")),
        "{page}"
    );
    assert!(
        page.contains(&format!("href=\"{squad}?tag=raid&#38;sort=level\"")),
        "{page}"
    );
    assert!(
        page.contains(&format!("href=\"{squad}?tag=raid\"")),
        "{page}"
    );

    // A form posted from that view comes back to it, on its own and through htmx.
    let id = member_id(&page, 0);
    let view = [("filter", "raid"), ("sort", "name")];
    let r = app
        .post(&format!("{squad}/characters/{id}/refresh"), &view)
        .await;
    assert_eq!(
        r.headers()[header::LOCATION],
        format!("{squad}?tag=raid&sort=name")
    );
    let (_, roster) = app
        .htmx(
            &format!("{squad}/characters/{id}/tags"),
            &[("tags", "alt"), view[0], view[1]],
        )
        .await;
    assert_eq!(member_names(&roster), ["Aldric", "Zora"]);
    assert!(roster.contains("name=\"sort\" value=\"name\""), "{roster}");
}

#[tokio::test]
async fn an_empty_squad_has_no_summary() {
    let app = app(false).await;
    let squad = app.squad("Raiders").await;
    let (_, page) = app.get(&squad).await;
    assert!(!page.contains("Sort by"), "{page}");
}

#[tokio::test]
async fn bad_input_is_explained_and_kept() {
    let app = app(false).await;
    let squad = app.squad("Raiders").await;

    let (status, roster) = app
        .htmx(
            &format!("{squad}/characters"),
            &[("character", "Aldric"), ("tags", "raid")],
        )
        .await;
    assert_eq!(status, StatusCode::OK, "htmx only swaps 2xx responses");
    assert!(roster.starts_with("<section id=\"roster\""), "{roster}");
    assert!(roster.contains("enter the character as Name-Surname"));
    assert!(roster.contains("value=\"Aldric\""), "keeps what was typed");

    let r = app.add(&squad, "A-Thornwood", "").await;
    assert_eq!(r.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert!(r.text().await.unwrap().contains("2 to 24 letters"));

    app.add(&squad, "Aldric-Thornwood", "").await;
    let (_, roster) = app
        .htmx(
            &format!("{squad}/characters"),
            &[("character", "aldric thornwood")],
        )
        .await;
    assert!(
        roster.contains("Aldric-Thornwood is already in this squad"),
        "{roster}"
    );
}

#[tokio::test]
async fn tags_filter_the_roster() {
    let app = app(false).await;
    let squad = app.squad("Raiders").await;
    app.add(&squad, "Aldric-Thornwood", "Raid, Main tank").await;
    app.add(&squad, "Bryn-Ashvale", "alts").await;

    let (_, all) = app.get(&squad).await;
    assert!(all.contains("Aldric") && all.contains("Bryn"));
    let (_, raid) = app.get(&format!("{squad}?tag=RAID")).await;
    assert!(raid.contains("Aldric") && !raid.contains("Bryn"), "{raid}");
    let (_, tank) = app.get(&format!("{squad}?tag=main%20tank")).await;
    assert!(tank.contains("Aldric") && !tank.contains("Bryn"));
    let (_, none) = app.get(&format!("{squad}?tag=healer")).await;
    assert!(none.contains("No one has that tag"));
}

#[tokio::test]
async fn tagging_and_untagging() {
    let app = app(false).await;
    let squad = app.squad("Raiders").await;
    app.add(&squad, "Aldric-Thornwood", "").await;
    let id = member_id(&app.get(&squad).await.1, 0);

    let (_, roster) = app
        .htmx(
            &format!("{squad}/characters/{id}/tags"),
            &[("tags", "Healer, raid")],
        )
        .await;
    assert!(
        roster.contains(">Healer<") && roster.contains(">raid<"),
        "{roster}"
    );

    let (_, roster) = app
        .htmx(&format!("{squad}/characters/{id}/tags/healer/delete"), &[])
        .await;
    assert!(
        !roster.contains(">Healer<") && roster.contains(">raid<"),
        "{roster}"
    );
}

/// The members part of a page, below the tag filter and colours.
fn members(page: &str) -> &str {
    page.split_once("<ul class=\"members\">")
        .map_or("", |(_, m)| m)
}

/// The chip for a tag on the first character that has it, from its opening tag to the end of
/// its link.
fn chip<'a>(page: &'a str, label: &str) -> Option<&'a str> {
    let page = members(page);
    let end = page.find(&format!(">{label}</a>"))?;
    let start = page[..end].rfind("class=\"chip tag")?;
    Some(&page[start..end])
}

#[tokio::test]
async fn armory_data_tags_characters_automatically() {
    let app = app(true).await;
    let squad = app.squad("Raiders").await;
    app.add(&squad, "Aldric-Thornwood", "raid, rogue").await;
    app.add(&squad, "Bryn-Ashvale", "").await;
    let page = app.wait_for(&squad, "Night Elf").await;

    for label in [
        "Rogue",
        "Combat",
        "Night Elf",
        "Alliance",
        "Skinning",
        "Cooking",
    ] {
        let chip = chip(&page, label).unwrap_or_else(|| panic!("no {label} chip in {page}"));
        assert!(chip.starts_with("class=\"chip tag auto"), "{chip}");
    }
    assert_eq!(
        members(&page).matches(">Rogue</a>").count(),
        1,
        "typed and automatic shown once"
    );
    assert!(
        !page.contains("/tags/rogue/delete"),
        "automatic tags can't be removed"
    );
    assert!(page.contains("/tags/raid/delete"), "typed ones can");
    assert!(
        chip(&page, "Rogue")
            .unwrap()
            .contains("style=\"--tag: #fff569; --tag-text: #000000\""),
        "class tags start in the class colour: {page}"
    );
    assert!(
        page.contains(&format!("href=\"{squad}?tag=night%20elf\" class=\"chip\"")),
        "automatic tags are in the filter: {page}"
    );

    let (_, filtered) = app.get(&format!("{squad}?tag=Skinning")).await;
    assert!(
        filtered.contains("Aldric") && !filtered.contains("Bryn"),
        "{filtered}"
    );
}

#[tokio::test]
async fn picking_and_resetting_tag_colours() {
    let app = app(false).await;
    let squad = app.squad("Raiders").await;
    let other = app.squad("Others").await;
    app.add(&squad, "Aldric-Thornwood", "Raid, Mage").await;
    app.add(&other, "Aldric-Thornwood", "raid").await;

    // Without htmx: a plain post and a redirect back.
    let r = app
        .post(
            &format!("{squad}/tags/raid/colour"),
            &[("colour", "#3FC7EB"), ("filter", "raid")],
        )
        .await;
    assert_eq!(r.status(), StatusCode::SEE_OTHER);
    assert_eq!(r.headers()[header::LOCATION], format!("{squad}?tag=raid"));
    let (_, page) = app.get(&squad).await;
    let raid = chip(&page, "Raid").unwrap();
    assert!(
        raid.contains("coloured") && raid.contains("--tag: #3fc7eb; --tag-text: #000000"),
        "{raid}"
    );
    assert!(
        page.contains("type=\"color\" name=\"colour\" value=\"#3fc7eb\""),
        "{page}"
    );
    assert!(
        !app.get(&other).await.1.contains("#3fc7eb"),
        "colours are per squad"
    );

    // A class tag starts in its class colour, can be changed and goes back on reset.
    let mage = "--tag: #69ccf0";
    assert!(chip(&page, "Mage").unwrap().contains(mage), "{page}");
    let (_, roster) = app
        .htmx(
            &format!("{squad}/tags/mage/colour"),
            &[("colour", "#000000")],
        )
        .await;
    assert!(
        chip(&roster, "Mage")
            .unwrap()
            .contains("--tag: #000000; --tag-text: #ffffff")
    );
    assert!(
        roster.contains("<details class=\"colours\" open>"),
        "stays open: {roster}"
    );
    let (_, roster) = app
        .htmx(
            &format!("{squad}/tags/mage/colour"),
            &[("colour", "#000000"), ("reset", "1")],
        )
        .await;
    assert!(chip(&roster, "Mage").unwrap().contains(mage), "{roster}");

    // Resetting a tag that isn't a class leaves it uncoloured.
    let (_, roster) = app
        .htmx(&format!("{squad}/tags/raid/colour"), &[("reset", "1")])
        .await;
    assert!(
        !chip(&roster, "Raid").unwrap().contains("coloured"),
        "{roster}"
    );

    assert!(
        roster.contains(">Add colour<"),
        "uncoloured tags offer to add one: {roster}"
    );

    let (status, roster) = app
        .htmx(&format!("{squad}/tags/raid/colour"), &[("colour", "red")])
        .await;
    assert_eq!(status, StatusCode::OK);
    assert!(roster.contains("pick a colour like #3fc7eb"), "{roster}");
    for colour in ["red", "#000000"] {
        let r = app
            .post(&format!("{squad}/tags/%20/colour"), &[("colour", colour)])
            .await;
        assert_eq!(r.status(), StatusCode::NOT_FOUND, "blank tag with {colour}");
    }
}

#[tokio::test]
async fn removing_a_character() {
    let app = app(false).await;
    let squad = app.squad("Raiders").await;
    app.add(&squad, "Aldric-Thornwood", "").await;
    let id = member_id(&app.get(&squad).await.1, 0);
    let r = app
        .post(&format!("{squad}/characters/{id}/delete"), &[])
        .await;
    assert_eq!(r.status(), StatusCode::SEE_OTHER);
    assert!(app.get(&squad).await.1.contains("No characters yet"));
}

#[tokio::test]
async fn squads_dont_touch_each_others_characters() {
    let app = app(false).await;
    let ours = app.squad("Ours").await;
    let theirs = app.squad("Theirs").await;
    app.add(&theirs, "Aldric-Thornwood", "").await;
    let id = member_id(&app.get(&theirs).await.1, 0);
    app.post(&format!("{ours}/characters/{id}/delete"), &[])
        .await;
    app.post(
        &format!("{ours}/characters/{id}/tags"),
        &[("tags", "hacked")],
    )
    .await;
    let (_, page) = app.get(&theirs).await;
    assert!(page.contains("Aldric") && !page.contains("hacked"));
}

#[tokio::test]
async fn without_credentials_names_and_tags_still_work() {
    let app = app(false).await;
    let squad = app.squad("Raiders").await;
    app.add(&squad, "Aldric-Thornwood", "raid").await;
    let (_, page) = app.get(&squad).await;
    assert!(page.contains("Armory lookups are off"));
    assert!(page.contains("Aldric") && page.contains(">raid<"));
    assert!(!page.contains("hx-trigger=\"every"), "nothing to poll for");
}

// ---- password ----

const PASSWORD: &str = "correct horse";

/// Logs in and returns the session cookie (`name=value`).
async fn log_in(app: &App, password: &str, next: &str) -> reqwest::Response {
    app.post("/login", &[("password", password), ("next", next)])
        .await
}

fn session(r: &reqwest::Response) -> String {
    r.headers()[header::SET_COOKIE]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned()
}

#[tokio::test]
async fn with_a_password_everything_but_health_and_static_needs_a_login() {
    let app = app_with(false, Some(PASSWORD)).await;
    let r = app
        .http
        .get(format!("{}/s/abc?tag=raid", app.base))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        r.headers()[header::LOCATION],
        "/login?next=%2Fs%2Fabc%3Ftag%3Draid"
    );
    assert_eq!(
        app.post("/squads", &[("name", "x")]).await.status(),
        StatusCode::SEE_OTHER
    );
    // Unknown paths too, so they don't reveal what exists.
    for path in ["/nope", "/loginx"] {
        let r = app
            .http
            .get(format!("{}{path}", app.base))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::SEE_OTHER, "{path}");
    }

    assert_eq!(app.get("/healthz").await.0, StatusCode::OK);
    assert_eq!(app.get("/version").await.0, StatusCode::OK);
    let (status, login) = app.get("/login?next=/s/abc").await;
    assert_eq!(status, StatusCode::OK);
    assert!(login.contains("type=\"password\"") && login.contains("value=\"/s/abc\""));
    let css = login
        .split("href=\"")
        .find_map(|s| s.split('"').next().filter(|h| h.ends_with("style.css")))
        .unwrap();
    assert_eq!(app.get(css).await.0, StatusCode::OK);
}

#[tokio::test]
async fn the_right_password_logs_in_and_back_to_where_you_were() {
    let app = app_with(false, Some(PASSWORD)).await;
    let r = log_in(&app, PASSWORD, "/?from=login").await;
    assert_eq!(r.status(), StatusCode::SEE_OTHER);
    assert_eq!(r.headers()[header::LOCATION], "/?from=login");
    let set_cookie = r.headers()[header::SET_COOKIE].to_str().unwrap().to_owned();
    assert!(
        set_cookie.contains("HttpOnly") && set_cookie.contains("SameSite=Lax"),
        "{set_cookie}"
    );
    let cookie = session(&r);

    let page = app
        .http
        .get(format!("{}/", app.base))
        .header(header::COOKIE, &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(page.status(), StatusCode::OK);
    assert!(page.text().await.unwrap().contains("Log out"));

    let created = app
        .http
        .post(format!("{}/squads", app.base))
        .header(header::COOKIE, &cookie)
        .form(&[("name", "Raiders")])
        .send()
        .await
        .unwrap();
    assert!(
        created.headers()[header::LOCATION]
            .to_str()
            .unwrap()
            .starts_with("/s/")
    );
}

#[tokio::test]
async fn a_wrong_password_or_cookie_gets_nowhere() {
    let app = app_with(false, Some(PASSWORD)).await;
    let r = log_in(&app, "Correct horse", "/").await;
    assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
    assert!(r.headers().get(header::SET_COOKIE).is_none());
    assert!(r.text().await.unwrap().contains("not the password"));

    let forged = app
        .http
        .get(format!("{}/", app.base))
        .header(header::COOKIE, "zesquad_session=00ff")
        .send()
        .await
        .unwrap();
    assert_eq!(forged.status(), StatusCode::SEE_OTHER);
}

#[tokio::test]
async fn login_only_redirects_within_the_site() {
    let app = app_with(false, Some(PASSWORD)).await;
    // Browsers drop the tab and newline, which would leave //evil.example.
    for next in [
        "https://evil.example/",
        "/\t/evil.example",
        "/\n/evil.example",
    ] {
        let r = log_in(&app, PASSWORD, next).await;
        assert_eq!(r.status(), StatusCode::SEE_OTHER, "{next:?}");
        assert_eq!(r.headers()[header::LOCATION], "/", "{next:?}");
    }
}

#[tokio::test]
async fn without_a_password_login_still_only_redirects_within_the_site() {
    let app = app(false).await;
    for next in ["/%09/evil.example", "/%0A/evil.example", "/%5Cevil.example"] {
        let r = app
            .http
            .get(format!("{}/login?next={next}", app.base))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::SEE_OTHER, "{next}");
        assert_eq!(r.headers()[header::LOCATION], "/", "{next}");
    }
}

#[tokio::test]
async fn htmx_requests_without_a_session_are_sent_to_the_login_page() {
    let app = app_with(false, Some(PASSWORD)).await;
    // Back to the page htmx was on, not the form's endpoint or the polled fragment.
    let r = app
        .http
        .post(format!("{}/s/abc/characters", app.base))
        .header("HX-Request", "true")
        .header("HX-Current-URL", format!("{}/s/abc?tag=raid", app.base))
        .form(&[("character", "Aldric-Thornwood")])
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    assert_eq!(
        r.headers()["hx-redirect"],
        "/login?next=%2Fs%2Fabc%3Ftag%3Draid"
    );
    let r = app
        .http
        .get(format!("{}/s/abc/roster", app.base))
        .header("HX-Request", "true")
        .header("HX-Current-URL", format!("{}/s/abc", app.base))
        .send()
        .await
        .unwrap();
    assert_eq!(r.headers()["hx-redirect"], "/login?next=%2Fs%2Fabc");

    // Without the header, the front page: the endpoint itself isn't a page.
    let r = app
        .http
        .post(format!("{}/s/abc/characters", app.base))
        .header("HX-Request", "true")
        .form(&[("character", "Aldric-Thornwood")])
        .send()
        .await
        .unwrap();
    assert_eq!(r.headers()["hx-redirect"], "/login?next=%2F");
}

#[tokio::test]
async fn a_form_post_without_a_session_returns_to_the_page_it_came_from() {
    let app = app_with(false, Some(PASSWORD)).await;
    let post = |referer: Option<String>| {
        let mut r = app
            .http
            .post(format!("{}/s/abc/characters", app.base))
            .form(&[("character", "Aldric-Thornwood")]);
        if let Some(referer) = referer {
            r = r.header(header::REFERER, referer);
        }
        r.send()
    };
    let r = post(Some(format!("{}/s/abc", app.base))).await.unwrap();
    assert_eq!(r.status(), StatusCode::SEE_OTHER);
    assert_eq!(r.headers()[header::LOCATION], "/login?next=%2Fs%2Fabc");
    let r = post(None).await.unwrap();
    assert_eq!(r.headers()[header::LOCATION], "/login?next=%2F");
    // Only the path of a Referer counts, and it must pass the same-site check.
    let r = post(Some("https://evil.example/%09/x".into()))
        .await
        .unwrap();
    assert_eq!(r.headers()[header::LOCATION], "/login?next=%2F%2509%2Fx");
}

#[tokio::test]
async fn logging_out_clears_the_session() {
    let app = app_with(false, Some(PASSWORD)).await;
    let r = app.post("/logout", &[]).await;
    assert_eq!(r.status(), StatusCode::SEE_OTHER);
    assert!(
        r.headers()[header::SET_COOKIE]
            .to_str()
            .unwrap()
            .contains("Max-Age=0")
    );
}

#[tokio::test]
async fn without_a_password_there_is_no_login() {
    let app = app(false).await;
    assert_eq!(app.get("/").await.0, StatusCode::OK);
    assert!(!app.get("/").await.1.contains("Log out"));
    let r = app
        .http
        .get(format!("{}/login?next=/s/x", app.base))
        .send()
        .await
        .unwrap();
    assert_eq!(r.headers()[header::LOCATION], "/s/x");
}

// ---- auction prices ----

// ---- logging in with Battle.net ----

/// The cookie named `name` that a response sets, as `name=value`.
fn set_cookie(r: &reqwest::Response, name: &str) -> Option<String> {
    r.headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find(|v| v.starts_with(&format!("{name}=")))
        .map(|v| v.split(';').next().unwrap().to_owned())
}

/// A started login: the login cookie and the `state` sent to Battle.net.
async fn start_login(app: &App, next: &str) -> (String, String) {
    let r = app
        .get_with(&format!("/auth/login?next={}", urlencode(next)), "")
        .await;
    assert_eq!(r.status(), StatusCode::SEE_OTHER);
    let to = reqwest::Url::parse(r.headers()[header::LOCATION].to_str().unwrap()).unwrap();
    let query: std::collections::HashMap<_, _> = to.query_pairs().into_owned().collect();
    assert!(to.path().ends_with("/authorize"), "{to}");
    assert_eq!(query["response_type"], "code");
    assert_eq!(query["client_id"], CLIENT_ID);
    // Nothing but who you are.
    assert_eq!(query["scope"], "openid");
    assert_eq!(query["redirect_uri"], format!("{PUBLIC_URL}/auth/callback"));
    let set = r
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .map(|v| v.to_str().unwrap().to_owned())
        .find(|v| v.starts_with("zesquad_login="))
        .unwrap();
    assert!(
        set.contains("HttpOnly") && set.contains("SameSite=Lax") && set.contains("; Secure"),
        "{set}"
    );
    (
        set_cookie(&r, "zesquad_login").unwrap(),
        query["state"].clone(),
    )
}

fn urlencode(s: &str) -> String {
    reqwest::Url::parse_with_params("http://x/", &[("v", s)])
        .unwrap()
        .query()
        .unwrap()
        .trim_start_matches("v=")
        .to_owned()
}

/// Logs in through the fake Battle.net and returns the session cookie.
async fn log_in_bnet(app: &App, next: &str) -> String {
    log_in_as(app, GOOD_CODE, next).await
}

/// Logs in as whoever `code` belongs to (`GOOD_CODE`, `FRIEND_CODE`, `STRANGER_CODE`).
async fn log_in_as(app: &App, code: &str, next: &str) -> String {
    let (cookie, state) = start_login(app, next).await;
    let r = app
        .get_with(
            &format!("/auth/callback?code={code}&state={state}"),
            &cookie,
        )
        .await;
    assert_eq!(r.status(), StatusCode::SEE_OTHER);
    assert_eq!(r.headers()[header::LOCATION], next);
    // The login cookie is cleared.
    assert_eq!(set_cookie(&r, "zesquad_login").unwrap(), "zesquad_login=");
    set_cookie(&r, "zesquad_user").unwrap()
}

#[tokio::test]
async fn logging_in_with_battle_net() {
    let app = app_with_login(None).await;
    let (_, page) = app.get("/auctions?x=1").await;
    assert!(
        page.contains("href=\"/auth/login?next=%2Fauctions%3Fx%3D1\""),
        "{page}"
    );
    let (_, front) = app.get("/").await;
    assert!(front.contains("Log in with Battle.net") && !front.contains("Log out"));

    let session = log_in_bnet(&app, "/auctions?x=1").await;
    let page = app.get_with("/", &session).await.text().await.unwrap();
    assert!(
        page.contains("Testhero#1234") && page.contains("Log out"),
        "{page}"
    );
    assert!(!page.contains("Log in with Battle.net"), "{page}");

    // The database keeps the account and a hash of the session, never the token itself.
    let db = zesquad::db::open(&app.dir.path().join("zesquad.db"), Region::Eu)
        .await
        .unwrap();
    let token = session.strip_prefix("zesquad_user=").unwrap();
    let (users, stored): (i64, String) =
        sqlx::query_as("SELECT (SELECT COUNT(*) FROM users), (SELECT token_hash FROM sessions)")
            .fetch_one(&db)
            .await
            .unwrap();
    assert_eq!(users, 1);
    assert_ne!(stored, token);
    assert_eq!(stored, zesquad::login::hash(token));

    // Logging in again is the same account.
    log_in_bnet(&app, "/").await;
    let users: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users")
        .fetch_one(&db)
        .await
        .unwrap();
    assert_eq!(users, 1);

    // Logging out ends the session for good.
    let r = app
        .http
        .post(format!("{}/logout", app.base))
        .header(header::COOKIE, &session)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::SEE_OTHER);
    assert_eq!(r.headers()[header::LOCATION], "/");
    assert_eq!(set_cookie(&r, "zesquad_user").unwrap(), "zesquad_user=");
    let page = app.get_with("/", &session).await.text().await.unwrap();
    assert!(!page.contains("Testhero#1234"), "{page}");
}

#[tokio::test]
async fn sessions_last_a_month_and_are_extended_when_used() {
    let app = app_with_login(None).await;
    let session = log_in_bnet(&app, "/").await;
    let db = zesquad::db::open(&app.dir.path().join("zesquad.db"), Region::Eu)
        .await
        .unwrap();
    let expires = || async {
        sqlx::query_scalar::<_, i64>("SELECT expires_at FROM sessions")
            .fetch_one(&db)
            .await
            .unwrap()
            - zesquad::db::now()
    };
    let month = 30 * 86_400;
    assert!((month - 60..=month).contains(&expires().await));
    // Fresh sessions aren't touched on every request.
    let r = app.get_with("/", &session).await;
    assert!(set_cookie(&r, "zesquad_user").is_none());

    // In its second half, using it starts another month.
    sqlx::query("UPDATE sessions SET expires_at = ?")
        .bind(zesquad::db::now() + 86_400)
        .execute(&db)
        .await
        .unwrap();
    let r = app.get_with("/", &session).await;
    assert_eq!(set_cookie(&r, "zesquad_user").unwrap(), session);
    assert!(expires().await > month - 60);

    // An expired one is nobody.
    sqlx::query("UPDATE sessions SET expires_at = ?")
        .bind(zesquad::db::now() - 1)
        .execute(&db)
        .await
        .unwrap();
    let page = app.get_with("/", &session).await.text().await.unwrap();
    assert!(!page.contains("Testhero#1234"), "{page}");
}

impl App {
    /// POSTs `path` with `cookie` and no form.
    async fn post_with(&self, path: &str, cookie: &str) -> reqwest::Response {
        self.http
            .post(format!("{}{path}", self.base))
            .header(header::COOKIE, cookie)
            .send()
            .await
            .unwrap()
    }
}

#[tokio::test]
async fn your_account_shows_what_is_kept_and_can_go() {
    let app = app_with_login(None).await;
    // Not logged in: log in first, then come back.
    let r = app.get_with("/account", "").await;
    assert_eq!(r.status(), StatusCode::SEE_OTHER);
    assert_eq!(r.headers()[header::LOCATION], "/auth/login?next=%2Faccount");

    let phone = log_in_bnet(&app, "/").await;
    let laptop = log_in_bnet(&app, "/").await;
    let page = app.get_with("/account", &phone).await.text().await.unwrap();
    assert!(
        page.contains("<strong>Testhero#1234</strong>, since just now"),
        "{page}"
    );
    assert!(page.contains("href=\"/account\""), "{page}");

    // Log out everywhere ends both sessions, keeps the account.
    let r = app.post_with("/account/sessions/delete", &phone).await;
    assert_eq!(r.status(), StatusCode::SEE_OTHER);
    for session in [&phone, &laptop] {
        let page = app.get_with("/", session).await.text().await.unwrap();
        assert!(!page.contains("Testhero#1234"));
    }
    let db = zesquad::db::open(&app.dir.path().join("zesquad.db"), Region::Eu)
        .await
        .unwrap();
    let count = |sql: &'static str| {
        let db = db.clone();
        async move {
            sqlx::query_scalar::<_, i64>(sql)
                .fetch_one(&db)
                .await
                .unwrap()
        }
    };
    assert_eq!(count("SELECT COUNT(*) FROM users").await, 1);
    assert_eq!(count("SELECT COUNT(*) FROM sessions").await, 0);

    // Deleting the account leaves nothing about you.
    let session = log_in_bnet(&app, "/").await;
    let r = app.post_with("/account/delete", &session).await;
    assert_eq!(r.status(), StatusCode::SEE_OTHER);
    assert_eq!(set_cookie(&r, "zesquad_user").unwrap(), "zesquad_user=");
    assert_eq!(count("SELECT COUNT(*) FROM users").await, 0);
    assert_eq!(count("SELECT COUNT(*) FROM sessions").await, 0);
    // Without a session, the account actions only send you to log in.
    let r = app.post_with("/account/delete", "").await;
    assert_eq!(r.status(), StatusCode::SEE_OTHER);
    assert!(
        r.headers()[header::LOCATION]
            .to_str()
            .unwrap()
            .starts_with("/auth/login")
    );
}

#[tokio::test]
async fn every_page_links_the_privacy_page() {
    let app = app(false).await;
    let (_, front) = app.get("/").await;
    assert!(front.contains("href=\"/privacy\""), "{front}");
    let (status, privacy) = app.get("/privacy").await;
    assert_eq!(status, StatusCode::OK);
    assert!(privacy.contains("account id") && privacy.contains("zesquad_recent"));
    // Without logging in there's no account to see.
    assert_eq!(app.get("/account").await.0, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_login_must_come_back_to_where_it_started() {
    let app = app_with_login(None).await;
    let (cookie, state) = start_login(&app, "/").await;
    let callback = |q: String, cookie: String| {
        let app = &app;
        async move { app.get_with(&format!("/auth/callback?{q}"), &cookie).await }
    };
    // Another state, no state, no login cookie: refused, nobody logged in.
    for (q, c) in [
        (format!("code={GOOD_CODE}&state=other"), cookie.clone()),
        (format!("code={GOOD_CODE}"), cookie.clone()),
        (format!("code={GOOD_CODE}&state={state}"), String::new()),
    ] {
        let r = callback(q.clone(), c).await;
        assert_eq!(r.status(), StatusCode::BAD_REQUEST, "{q}");
        assert!(set_cookie(&r, "zesquad_user").is_none(), "{q}");
        assert!(r.text().await.unwrap().contains("Log in again"));
    }
    // Declining on Battle.net's page.
    let r = callback(format!("error=access_denied&state={state}"), cookie.clone()).await;
    assert_eq!(r.status(), StatusCode::FORBIDDEN);
    assert!(r.text().await.unwrap().contains("nothing was shared"));
    // A code Battle.net doesn't accept.
    let r = callback(format!("code=bad&state={state}"), cookie.clone()).await;
    assert_eq!(r.status(), StatusCode::BAD_GATEWAY);
    assert!(set_cookie(&r, "zesquad_user").is_none());
    // A made-up session cookie is nobody.
    let page = app
        .get_with("/", "zesquad_user=0123456789abcdef")
        .await
        .text()
        .await
        .unwrap();
    assert!(page.contains("Log in with Battle.net"), "{page}");
    // Somewhere else to return to is the front page.
    let session_cookie = {
        let (cookie, state) = start_login(&app, "//evil.example").await;
        let r = app
            .get_with(
                &format!("/auth/callback?code={GOOD_CODE}&state={state}"),
                &cookie,
            )
            .await;
        assert_eq!(r.headers()[header::LOCATION], "/");
        set_cookie(&r, "zesquad_user")
    };
    assert!(session_cookie.is_some());
}

#[tokio::test]
async fn without_a_public_url_there_is_no_battle_net_login() {
    let app = app(true).await;
    let (_, page) = app.get("/").await;
    assert!(!page.contains("Log in with Battle.net"), "{page}");
    assert_eq!(app.get("/auth/login").await.0, StatusCode::NOT_FOUND);
    assert_eq!(
        app.get("/auth/callback?code=x&state=y").await.0,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn logging_out_ends_both_the_password_and_the_battle_net_session() {
    let app = app_with_login(Some(PASSWORD)).await;
    let password = session(&log_in(&app, PASSWORD, "/").await);
    let bnet = {
        let r = app.get_with("/auth/login?next=/", &password).await;
        let to = reqwest::Url::parse(r.headers()[header::LOCATION].to_str().unwrap()).unwrap();
        let state = to
            .query_pairs()
            .find(|(k, _)| k == "state")
            .unwrap()
            .1
            .into_owned();
        let login = set_cookie(&r, "zesquad_login").unwrap();
        let r = app
            .get_with(
                &format!("/auth/callback?code={GOOD_CODE}&state={state}"),
                &format!("{password}; {login}"),
            )
            .await;
        set_cookie(&r, "zesquad_user").unwrap()
    };
    let both = format!("{password}; {bnet}");
    let page = app.get_with("/", &both).await.text().await.unwrap();
    assert!(page.contains("Testhero#1234"), "{page}");
    let r = app
        .http
        .post(format!("{}/logout", app.base))
        .header(header::COOKIE, &both)
        .send()
        .await
        .unwrap();
    assert_eq!(r.headers()[header::LOCATION], "/login");
    assert_eq!(set_cookie(&r, "zesquad_user").unwrap(), "zesquad_user=");
    assert_eq!(
        set_cookie(&r, "zesquad_session").unwrap(),
        "zesquad_session="
    );
}

#[tokio::test]
async fn auction_prices_are_followed_in_eu_and_us() {
    use zesquad::db;
    use zesquad_core::auctions::ItemPrices;

    let app = app_with_auctions(true, None, true).await;
    let db = db::open(&app.dir.path().join("zesquad.db"), Region::Eu)
        .await
        .unwrap();
    let followed = [110, 111, 113, 114];
    let mut checked = false;
    for _ in 0..100 {
        let mut all = true;
        for region in [Region::Eu, Region::Us] {
            for house in followed {
                all &= db::auctions_checked_at(&db, region, house)
                    .await
                    .unwrap()
                    .is_some();
            }
        }
        if all {
            checked = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(checked, "every followed house gets checked");

    // Items and commodities make one snapshot of the house.
    for region in [Region::Eu, Region::Us] {
        let (_, prices) = db::latest_auctions(&db, region, 110)
            .await
            .unwrap()
            .unwrap();
        let ids: Vec<u32> = prices.iter().map(|p| p.item).collect();
        assert_eq!(ids, [2318, 2589, 2592, 4306, 11208, 19019], "{region}");
        assert_eq!(
            prices[1],
            ItemPrices {
                item: 2589,
                min_price: 10,
                median_price: 10,
                quantity: 26,
                auctions: 3
            }
        );
    }
    // Empty houses have nothing to show; a house that's down doesn't stop the others.
    assert_eq!(
        db::latest_auctions(&db, Region::Eu, 114).await.unwrap(),
        None
    );
    assert_eq!(
        db::latest_auctions(&db, Region::Us, 111).await.unwrap(),
        None
    );

    let mut requests = app.fake.auction_requests.lock().unwrap().clone();
    requests.sort();
    assert!(requests.contains(&"us/114/commodities".to_owned()));
    assert!(
        !requests.contains(&"us/111/commodities".to_owned()),
        "stops at the failure"
    );
    assert!(
        requests.iter().all(|r| !r.starts_with("tw/")),
        "Taiwan isn't followed"
    );
    assert!(
        requests
            .iter()
            .all(|r| ["110", "111", "113", "114"].contains(&&r[3..6])),
        "only the followed houses: {requests:?}"
    );
    assert_eq!(
        requests.len(),
        15,
        "both dumps of each house, once: {requests:?}"
    );

    // The page shows the collecting and the leather prices.
    let (status, page) = app.get("/auctions").await;
    assert_eq!(status, StatusCode::OK);
    for shown in [
        "Europe",
        "Americas",
        "Alliance PvP",
        "Horde PvE",
        "house 110",
        "Light Leather",
        "Rugged Leather",
        // 45 copper, shown with a coin, and a line of the one snapshot so far.
        "<span class=\"coin copper\">45<span class=\"visually-hidden\">c</span></span>",
        "<svg class=\"spark\"",
        "7 days: average 45c, lowest 45c, highest 45c, 1 snapshots",
        "503 Service Unavailable",
        "no data",
    ] {
        assert!(page.contains(shown), "missing {shown:?} in {page}");
    }
    assert!(!page.contains("collection is off"));
}

#[tokio::test]
async fn followed_auction_dumps_are_archived() {
    let app = app_with_auctions(true, None, true).await;
    // Every dump fetched is uploaded after the round; US 111 is down.
    let mut archived = Vec::new();
    for _ in 0..100 {
        archived = app.fake.archived.lock().unwrap().keys().cloned().collect();
        if archived.len() >= 14 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(archived.len(), 14, "{archived:?}");
    assert!(
        archived.contains(&"zesquad/us/110/2026-11-20/20261120T202348Z-items.json.zst".to_owned()),
        "{archived:?}"
    );
    assert!(!archived.iter().any(|k| k.starts_with("zesquad/us/111/")));
}

#[tokio::test]
async fn new_auction_dumps_are_archived_until_uploaded() {
    use zesquad::archive::Archive;
    use zesquad::auctions::AuctionSync;
    use zesquad::db;
    use zesquad_core::auctions::followed;

    let (fake_url, fake) = start_fake().await;
    let dir = tempfile::tempdir().unwrap();
    let db = db::open(&dir.path().join("zesquad.db"), Region::Eu)
        .await
        .unwrap();
    let config = zesquad::armory::Config::new(
        CLIENT_ID.into(),
        CLIENT_SECRET.into(),
        Some(&fake_url),
        &format!("{fake_url}/token"),
    )
    .unwrap();
    let spool = dir.path().join("archive");
    let archive = Arc::new(Archive::new(archive_config(&fake_url, &spool)).unwrap());
    let sync = AuctionSync {
        db: db.clone(),
        client: Arc::new(zesquad::armory::Client::new(config).unwrap()),
        archive: Some(archive.clone()),
    };
    let eu = |id| {
        followed()
            .find(|h| h.region == Region::Eu && h.id == id)
            .unwrap()
    };
    let waiting = |key: &str| spool.join(key);

    // Each new dump is kept as sent, named by its Last-Modified, until it's uploaded.
    fake.archive_down.store(true, Ordering::SeqCst);
    sync.update(eu(110), 1_000).await.unwrap();
    let items = "eu/110/2026-11-20/20261120T202348Z-items.json.zst";
    let commodities = "eu/110/2026-11-20/20261120T202348Z-commodities.json.zst";
    assert_eq!(
        unzstd(&std::fs::read(waiting(items)).unwrap()),
        fixture("auctions")
    );
    assert_eq!(
        unzstd(&std::fs::read(waiting(commodities)).unwrap()),
        fixture("commodities")
    );

    // While the bucket is down, they wait.
    assert!(archive.upload().await.is_err());
    assert!(waiting(items).exists());
    assert!(fake.archived.lock().unwrap().is_empty());

    // Then they're uploaded and removed, day folder and all.
    fake.archive_down.store(false, Ordering::SeqCst);
    assert_eq!(archive.upload().await.unwrap(), 2);
    assert!(!waiting(items).exists() && !waiting(commodities).exists());
    assert!(!spool.join("eu/110/2026-11-20").exists());
    {
        let archived = fake.archived.lock().unwrap();
        assert_eq!(
            unzstd(&archived[&format!("zesquad/{items}")]),
            fixture("auctions")
        );
        assert_eq!(archived.len(), 2);
    }

    // A dump that hasn't changed isn't kept again.
    sync.update(eu(110), 2_000).await.unwrap();
    assert_eq!(archive.upload().await.unwrap(), 0);

    // Nor one only fetched again to complete a snapshot: when only EU 113's commodities
    // changed, only they are new.
    sync.update(eu(113), 1_000).await.unwrap();
    assert_eq!(archive.upload().await.unwrap(), 2);
    sync.update(eu(113), 2_000).await.unwrap();
    assert!(waiting("eu/113/2026-11-20/20261120T212348Z-commodities.json.zst").exists());
    assert!(!waiting("eu/113/2026-11-20/20261120T202348Z-items.json.zst").exists());
    assert_eq!(archive.upload().await.unwrap(), 1);
    let archived: Vec<String> = fake.archived.lock().unwrap().keys().cloned().collect();
    assert_eq!(
        archived,
        [
            "zesquad/eu/110/2026-11-20/20261120T202348Z-commodities.json.zst",
            "zesquad/eu/110/2026-11-20/20261120T202348Z-items.json.zst",
            "zesquad/eu/113/2026-11-20/20261120T202348Z-items.json.zst",
            "zesquad/eu/113/2026-11-20/20261120T212348Z-commodities.json.zst",
        ]
    );
    assert_eq!(archive.upload().await.unwrap(), 0, "nothing left");
}

#[tokio::test]
async fn unchanged_auction_dumps_are_not_downloaded_again() {
    use zesquad::auctions::AuctionSync;
    use zesquad::db;
    use zesquad_core::auctions::followed;

    let (fake_url, fake) = start_fake().await;
    let dir = tempfile::tempdir().unwrap();
    let db = db::open(&dir.path().join("zesquad.db"), Region::Eu)
        .await
        .unwrap();
    let config = zesquad::armory::Config::new(
        CLIENT_ID.into(),
        CLIENT_SECRET.into(),
        Some(&fake_url),
        &format!("{fake_url}/token"),
    )
    .unwrap();
    let sync = AuctionSync {
        db: db.clone(),
        client: Arc::new(zesquad::armory::Client::new(config).unwrap()),
        archive: None,
    };
    let eu = |id| {
        followed()
            .find(|h| h.region == Region::Eu && h.id == id)
            .unwrap()
    };
    let requests = || std::mem::take(&mut *fake.auction_requests.lock().unwrap());

    // The first time, both dumps are fetched plainly and their Last-Modified kept.
    sync.update(eu(110), 1_000).await.unwrap();
    assert_eq!(requests(), ["eu/110", "eu/110/commodities"]);
    assert_eq!(
        db::auctions_modified(&db, Region::Eu, 110).await.unwrap(),
        db::DumpsModified {
            items: Some(DUMPS_MODIFIED.into()),
            commodities: Some(DUMPS_MODIFIED.into()),
        }
    );

    // Then Battle.net is asked whether they changed; they haven't, so nothing new is stored.
    sync.update(eu(110), 2_000).await.unwrap();
    assert_eq!(
        requests(),
        [
            "eu/110 if-modified-since",
            "eu/110/commodities if-modified-since"
        ]
    );
    let status = db::house_status(&db, Region::Eu, 110).await.unwrap();
    assert_eq!((status.snapshots, status.checked_at), (1, Some(2_000)));
    assert_eq!(status.error, None);

    // When only one dump changed, the other is fetched again to make a whole snapshot.
    sync.update(eu(113), 1_000).await.unwrap();
    requests();
    sync.update(eu(113), 2_000).await.unwrap();
    assert_eq!(
        requests(),
        [
            "eu/113 if-modified-since",
            "eu/113/commodities if-modified-since",
            "eu/113"
        ]
    );
    assert_eq!(
        db::house_status(&db, Region::Eu, 113)
            .await
            .unwrap()
            .checked_at,
        Some(2_000)
    );
}

#[tokio::test]
async fn the_auction_page_is_linked_and_says_when_collecting_is_off() {
    let app = app(false).await;
    let (_, home) = app.get("/").await;
    assert!(home.contains("href=\"/auctions\""));
    let (status, page) = app.get("/auctions").await;
    assert_eq!(status, StatusCode::OK);
    assert!(page.contains("Price collection is off"), "{page}");
    assert!(page.contains("never"), "nothing checked yet: {page}");
}

#[tokio::test]
async fn auction_prices_are_off_unless_asked_for() {
    let app = app(true).await;
    let squad = app.squad("Raiders").await;
    app.add(&squad, "aldric-thornwood", "").await;
    app.wait_for(&squad, "Night Elf").await;
    assert_eq!(
        *app.fake.auction_requests.lock().unwrap(),
        Vec::<String>::new()
    );
}

// ---- owners, members and visibility ----

impl App {
    /// The app's database, opened next to it.
    async fn db(&self) -> sqlx::SqlitePool {
        zesquad::db::open(&self.dir.path().join("zesquad.db"), Region::Eu)
            .await
            .unwrap()
    }

    /// POSTs a form with `cookie`.
    async fn post_as(&self, cookie: &str, path: &str, form: &[(&str, &str)]) -> reqwest::Response {
        self.http
            .post(format!("{}{path}", self.base))
            .header(header::COOKIE, cookie)
            .form(form)
            .send()
            .await
            .unwrap()
    }

    /// Posts like htmx does, with `cookie`.
    async fn htmx_as(&self, cookie: &str, path: &str, form: &[(&str, &str)]) -> reqwest::Response {
        self.http
            .post(format!("{}{path}", self.base))
            .header(header::COOKIE, cookie)
            .header("HX-Request", "true")
            .form(form)
            .send()
            .await
            .unwrap()
    }

    /// GETs a page with `cookie`: its status and text.
    async fn page_as(&self, cookie: &str, path: &str) -> (StatusCode, String) {
        let r = self.get_with(path, cookie).await;
        (r.status(), r.text().await.unwrap())
    }

    /// Creates a squad as whoever `cookie` logs in, and returns its path.
    async fn squad_as(&self, cookie: &str, name: &str) -> String {
        let r = self.post_as(cookie, "/squads", &[("name", name)]).await;
        assert_eq!(r.status(), StatusCode::SEE_OTHER);
        r.headers()[header::LOCATION].to_str().unwrap().to_owned()
    }
}

#[tokio::test]
async fn starting_a_squad_takes_a_login_and_makes_you_its_owner() {
    let app = app_with_login(None).await;
    let (_, front) = app.get("/").await;
    assert!(
        front.contains("Log in with Battle.net to start a squad"),
        "{front}"
    );
    assert!(!front.contains("action=\"/squads\""), "{front}");
    let r = app.post("/squads", &[("name", "Sneaky")]).await;
    assert_eq!(r.status(), StatusCode::SEE_OTHER);
    assert_eq!(r.headers()[header::LOCATION], "/auth/login?next=%2F");
    let db = app.db().await;
    let squads: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM squads")
        .fetch_one(&db)
        .await
        .unwrap();
    assert_eq!(squads, 0);

    let me = log_in_bnet(&app, "/").await;
    let (_, front) = app.page_as(&me, "/").await;
    assert!(front.contains("action=\"/squads\""), "{front}");
    let squad = app.squad_as(&me, "Thursday raiders").await;
    let id = squad.strip_prefix("/s/").unwrap();
    let (visibility, token): (String, String) =
        sqlx::query_as("SELECT visibility, invite_token FROM squads WHERE id = ?")
            .bind(id)
            .fetch_one(&db)
            .await
            .unwrap();
    assert_eq!(visibility, "unlisted");
    assert_eq!(token.len(), 64);
    let members: Vec<(String, String)> = sqlx::query_as(
        "SELECT u.battletag, m.role FROM squad_members m JOIN users u ON u.id = m.user_id
         WHERE m.squad_id = ?",
    )
    .bind(id)
    .fetch_all(&db)
    .await
    .unwrap();
    assert_eq!(members, [("Testhero#1234".to_owned(), "owner".to_owned())]);
}

#[tokio::test]
async fn without_accounts_squads_are_open_and_nobodys() {
    let app = app(false).await;
    let (_, front) = app.get("/").await;
    assert!(!front.contains("to start a squad"), "{front}");
    let squad = app.squad("Thursday raiders").await;
    let db = app.db().await;
    let (visibility, token): (String, Option<String>) =
        sqlx::query_as("SELECT visibility, invite_token FROM squads WHERE id = ?")
            .bind(squad.strip_prefix("/s/").unwrap())
            .fetch_one(&db)
            .await
            .unwrap();
    assert_eq!((visibility.as_str(), token), ("open", None));
    let members: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM squad_members")
        .fetch_one(&db)
        .await
        .unwrap();
    assert_eq!(members, 0);
}

/// Every form a squad page can post, below the squad's path, for character `id` tagged `raid`.
fn squad_posts(id: &str) -> Vec<(String, Vec<(&'static str, &'static str)>)> {
    vec![
        ("/name".into(), vec![("name", "Renamed")]),
        (
            "/discord".into(),
            vec![("webhook", "https://discord.com/api/webhooks/1/x")],
        ),
        ("/discord/delete".into(), vec![]),
        ("/characters".into(), vec![("character", "Bryn-Ashvale")]),
        (format!("/characters/{id}/tags"), vec![("tags", "tank")]),
        (format!("/characters/{id}/tags/raid/delete"), vec![]),
        ("/tags/raid/colour".into(), vec![("colour", "#ff0000")]),
        (format!("/characters/{id}/refresh"), vec![]),
        (format!("/characters/{id}/delete"), vec![]),
    ]
}

/// What a squad looks like in the database: its name and characters with their tags.
async fn squad_state(db: &sqlx::SqlitePool, squad: &str) -> String {
    let id = squad.strip_prefix("/s/").unwrap();
    let rows: Vec<(String, String, Option<String>)> = sqlx::query_as(
        "SELECT s.name || ' ' || COALESCE(s.discord_webhook, ''), COALESCE(c.char_key, ''),
                (SELECT group_concat(tag_key) FROM character_tags t WHERE t.character_id = c.id)
         FROM squads s LEFT JOIN characters c ON c.squad_id = s.id WHERE s.id = ? ORDER BY c.id",
    )
    .bind(id)
    .fetch_all(db)
    .await
    .unwrap();
    let colours: Vec<String> =
        sqlx::query_scalar("SELECT tag_key || colour FROM tag_colours WHERE squad_id = ?")
            .bind(id)
            .fetch_all(db)
            .await
            .unwrap();
    format!("{rows:?} {colours:?}")
}

#[tokio::test]
async fn only_members_change_a_squad() {
    let app = app_with_login(None).await;
    let owner = log_in_as(&app, GOOD_CODE, "/").await;
    let stranger = log_in_as(&app, STRANGER_CODE, "/").await;
    let squad = app.squad_as(&owner, "Thursday raiders").await;
    let r = app
        .post_as(
            &owner,
            &format!("{squad}/characters"),
            &[("character", "Aldric-Thornwood"), ("tags", "raid")],
        )
        .await;
    assert_eq!(r.status(), StatusCode::SEE_OTHER);
    let (_, page) = app.page_as(&owner, &squad).await;
    let id = member_id(&page, 0);
    assert!(
        page.contains("action=\"/s/") && page.contains("/characters\""),
        "{page}"
    );
    let settings = format!("{squad}/settings");
    let (_, page) = app.page_as(&owner, &settings).await;
    assert!(page.contains("Rename"), "{page}");
    let db = app.db().await;
    let before = squad_state(&db, &squad).await;

    // Logged out or not in the squad: the roster, read-only, and every change refused.
    for who in ["", stranger.as_str()] {
        let (status, page) = app.page_as(who, &squad).await;
        assert_eq!(status, StatusCode::OK);
        assert!(
            page.contains("Thursday raiders") && page.contains("Aldric"),
            "{page}"
        );
        assert!(!page.contains("method=\"post\" action=\"/s/"), "{page}");
        assert!(
            page.contains("Ask a member for the invite link to edit"),
            "{page}"
        );
        assert!(!page.contains("/settings\""), "{page}");
        assert_eq!(app.page_as(who, &settings).await.0, StatusCode::FORBIDDEN);
        let (status, partial) = app.page_as(who, &format!("{squad}/roster")).await;
        assert_eq!(status, StatusCode::OK);
        assert!(!partial.contains("method=\"post\""), "{partial}");
        for (path, form) in squad_posts(&id) {
            let r = app.post_as(who, &format!("{squad}{path}"), &form).await;
            assert_eq!(r.status(), StatusCode::FORBIDDEN, "{path}");
            assert!(r.text().await.unwrap().contains("invite link"), "{path}");
            // htmx can't swap a 403, so it's told to reload, which shows the read-only page.
            let r = app.htmx_as(who, &format!("{squad}{path}"), &form).await;
            assert_eq!(r.status(), StatusCode::OK, "{path}");
            assert_eq!(r.headers()["hx-refresh"], "true", "{path}");
        }
    }
    assert_eq!(squad_state(&db, &squad).await, before);

    // A member edits the roster, but renaming and Discord are the owner's.
    let friend = log_in_as(&app, FRIEND_CODE, "/").await;
    sqlx::query(
        "INSERT INTO squad_members (squad_id, user_id, role, joined_at)
         SELECT ?, id, 'member', 0 FROM users WHERE battletag = 'Friendly#5678'",
    )
    .bind(squad.strip_prefix("/s/").unwrap())
    .execute(&db)
    .await
    .unwrap();
    let (_, page) = app.page_as(&friend, &squad).await;
    assert!(page.contains("/characters\""), "{page}");
    assert!(!page.contains("Ask a member"), "{page}");
    let (status, page) = app.page_as(&friend, &settings).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        !page.contains("Rename") && !page.contains("Discord"),
        "{page}"
    );
    for (path, form) in squad_posts(&id) {
        let r = app.post_as(&friend, &format!("{squad}{path}"), &form).await;
        let expected = if path.starts_with("/name") || path.starts_with("/discord") {
            StatusCode::FORBIDDEN
        } else {
            StatusCode::SEE_OTHER
        };
        assert_eq!(r.status(), expected, "{path}");
    }
    let r = app
        .post_as(&owner, &format!("{squad}/name"), &[("name", "Renamed")])
        .await;
    assert_eq!(r.status(), StatusCode::SEE_OTHER);
    let (_, page) = app.page_as("", &squad).await;
    assert!(page.contains("Renamed"), "{page}");
}

#[tokio::test]
async fn a_private_squad_is_not_there_for_anyone_else() {
    let app = app_with_login(None).await;
    let owner = log_in_as(&app, GOOD_CODE, "/").await;
    let stranger = log_in_as(&app, STRANGER_CODE, "/").await;
    let squad = app.squad_as(&owner, "Secret raiders").await;
    let db = app.db().await;
    sqlx::query("UPDATE squads SET visibility = 'private'")
        .execute(&db)
        .await
        .unwrap();
    assert_eq!(app.page_as(&owner, &squad).await.0, StatusCode::OK);
    for who in ["", stranger.as_str()] {
        let (status, page) = app.page_as(who, &squad).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert!(!page.contains("Secret raiders"), "{page}");
        assert_eq!(
            app.page_as(who, &format!("{squad}/roster")).await.0,
            StatusCode::NOT_FOUND
        );
        for (path, form) in squad_posts("1") {
            let r = app.post_as(who, &format!("{squad}{path}"), &form).await;
            assert_eq!(r.status(), StatusCode::NOT_FOUND, "{path}");
        }
    }
}

#[tokio::test]
async fn open_squads_stay_editable_by_anyone_with_the_link() {
    let app = app_with_login(None).await;
    let db = app.db().await;
    sqlx::query(
        "INSERT INTO squads (id, name, region, created_at) VALUES ('oldsquad', 'Old raiders', 'eu', 0)",
    )
    .execute(&db)
    .await
    .unwrap();
    let r = app.add("/s/oldsquad", "Aldric-Thornwood", "").await;
    assert_eq!(r.status(), StatusCode::SEE_OTHER);
    let r = app
        .post("/s/oldsquad/name", &[("name", "Older raiders")])
        .await;
    assert_eq!(r.status(), StatusCode::SEE_OTHER);
    let (_, page) = app.get("/s/oldsquad").await;
    assert!(
        page.contains("Older raiders") && page.contains("see and edit"),
        "{page}"
    );
}

#[tokio::test]
async fn with_accounts_off_only_open_squads_change_and_private_ones_stay_hidden() {
    let app = app(false).await;
    let open = app.squad("Thursday raiders").await;
    let private = app.squad("Secret raiders").await;
    let unlisted = app.squad("Quiet raiders").await;
    let db = app.db().await;
    // As if made while accounts were on, which are now off.
    for (squad, visibility) in [(&private, "private"), (&unlisted, "unlisted")] {
        sqlx::query("UPDATE squads SET visibility = ? WHERE id = ?")
            .bind(visibility)
            .bind(squad.strip_prefix("/s/").unwrap())
            .execute(&db)
            .await
            .unwrap();
    }
    let r = app.add(&open, "Aldric-Thornwood", "").await;
    assert_eq!(r.status(), StatusCode::SEE_OTHER);
    let (_, page) = app.get(&format!("{open}/settings")).await;
    assert!(
        page.contains("Rename") && page.contains("Discord"),
        "{page}"
    );
    // Nobody's a member, so there's no invite link or members list.
    assert!(
        !page.contains("Invite link") && !page.contains("Members ("),
        "{page}"
    );

    let (status, page) = app.get(&private).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(!page.contains("Secret raiders"), "{page}");
    assert_eq!(
        app.add(&private, "Aldric-Thornwood", "").await.status(),
        StatusCode::NOT_FOUND
    );

    let (status, page) = app.get(&unlisted).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        !page.contains("/settings\"") && page.contains("Quiet raiders"),
        "{page}"
    );
    assert_eq!(
        app.get(&format!("{unlisted}/settings")).await.0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        app.add(&unlisted, "Aldric-Thornwood", "").await.status(),
        StatusCode::FORBIDDEN
    );

    // Nor does the front page list the private one from the browser's history.
    let ids = [&private, &unlisted]
        .map(|p| p.strip_prefix("/s/").unwrap().to_owned())
        .join(".");
    let (_, front) = app.page_as(&format!("zesquad_recent={ids}"), "/").await;
    assert!(
        front.contains("Quiet raiders") && !front.contains("Secret raiders"),
        "{front}"
    );
}

/// The invite link on a squad's settings page, as a path: `/join/<token>`.
fn invite_path(page: &str) -> String {
    let link = page
        .split("id=\"invite-link\" value=\"")
        .nth(1)
        .and_then(|s| s.split('"').next())
        .unwrap();
    format!("/join/{}", link.rsplit_once("/join/").unwrap().1)
}

#[tokio::test]
async fn friends_join_with_the_invite_link() {
    let app = app_with_login(None).await;
    let owner = log_in_as(&app, GOOD_CODE, "/").await;
    let squad = app.squad_as(&owner, "Thursday raiders").await;
    let settings = format!("{squad}/settings");
    let (_, page) = app.page_as(&owner, &squad).await;
    assert!(page.contains("Anyone with this link can look"), "{page}");
    assert!(!page.contains("/join/"), "{page}");
    let (_, page) = app.page_as(&owner, &settings).await;
    assert!(
        page.contains("Anyone with this link can join and edit"),
        "{page}"
    );
    let invite = invite_path(&page);
    assert_eq!(invite.len(), "/join/".len() + 64, "{invite}");
    // Outsiders don't see it.
    let (_, outside) = app.page_as("", &squad).await;
    assert!(!outside.contains("/join/"), "{outside}");

    // Logged out: log in first, then back to the invite.
    let r = app.get_with(&invite, "").await;
    assert_eq!(r.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        r.headers()[header::LOCATION],
        format!("/auth/login?next={}", urlencode(&invite))
    );
    let friend = log_in_as(&app, FRIEND_CODE, &invite).await;
    // Opening it only asks; it never joins by itself.
    let (status, page) = app.page_as(&friend, &invite).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        page.contains("Join <em>Thursday raiders</em>?") && page.contains("Friendly#5678"),
        "{page}"
    );
    let r = app
        .post_as(
            &friend,
            &format!("{squad}/characters"),
            &[("character", "Bryn-Ashvale")],
        )
        .await;
    assert_eq!(r.status(), StatusCode::FORBIDDEN);
    let r = app.post_as(&friend, &invite, &[]).await;
    assert_eq!(r.status(), StatusCode::SEE_OTHER);
    assert_eq!(r.headers()[header::LOCATION], squad.as_str());
    let r = app
        .post_as(
            &friend,
            &format!("{squad}/characters"),
            &[("character", "Bryn-Ashvale")],
        )
        .await;
    assert_eq!(r.status(), StatusCode::SEE_OTHER);
    // Members pass the link on too, but can't reset it.
    let (_, page) = app.page_as(&friend, &settings).await;
    assert_eq!(invite_path(&page), invite);
    assert!(!page.contains("/invite/reset"), "{page}");
    // Already in: the invite goes straight to the squad, and joining again changes nothing.
    let r = app.get_with(&invite, &friend).await;
    assert_eq!(r.status(), StatusCode::SEE_OTHER);
    assert_eq!(r.headers()[header::LOCATION], squad.as_str());
    app.post_as(&owner, &invite, &[]).await;
    let db = app.db().await;
    let roles: Vec<String> =
        sqlx::query_scalar("SELECT role FROM squad_members ORDER BY joined_at, role DESC")
            .fetch_all(&db)
            .await
            .unwrap();
    assert_eq!(roles, ["owner", "member"]);

    // A made-up token is nothing.
    assert_eq!(
        app.page_as(&friend, "/join/nope").await.0,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn the_owner_can_reset_the_invite_link() {
    let app = app_with_login(None).await;
    let owner = log_in_as(&app, GOOD_CODE, "/").await;
    let friend = log_in_as(&app, FRIEND_CODE, "/").await;
    let squad = app.squad_as(&owner, "Thursday raiders").await;
    let old = invite_path(&app.page_as(&owner, &format!("{squad}/settings")).await.1);
    app.post_as(&friend, &old, &[]).await;
    let r = app
        .post_as(&friend, &format!("{squad}/invite/reset"), &[])
        .await;
    assert_eq!(r.status(), StatusCode::FORBIDDEN);
    let r = app
        .post_as(&owner, &format!("{squad}/invite/reset"), &[])
        .await;
    assert_eq!(r.status(), StatusCode::SEE_OTHER);
    let new = invite_path(&app.page_as(&owner, &format!("{squad}/settings")).await.1);
    assert_ne!(new, old);
    let stranger = log_in_as(&app, STRANGER_CODE, "/").await;
    assert_eq!(app.page_as(&stranger, &old).await.0, StatusCode::NOT_FOUND);
    assert_eq!(
        app.post_as(&stranger, &old, &[]).await.status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(app.page_as(&stranger, &new).await.0, StatusCode::OK);
    // Whoever joined stays.
    let r = app
        .post_as(
            &friend,
            &format!("{squad}/characters"),
            &[("character", "Bryn-Ashvale")],
        )
        .await;
    assert_eq!(r.status(), StatusCode::SEE_OTHER);
}

#[tokio::test]
async fn without_accounts_there_are_no_invites() {
    let app = app(false).await;
    let squad = app.squad("Thursday raiders").await;
    let (_, page) = app.get(&squad).await;
    assert!(!page.contains("/join/"), "{page}");
    assert_eq!(app.get("/join/anything").await.0, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn squads_from_before_accounts_go_to_whoever_opens_them_first() {
    let app = app_with_login(None).await;
    let db = app.db().await;
    sqlx::query(
        "INSERT INTO squads (id, name, region, created_at) VALUES ('oldsquad', 'Old raiders', 'eu', 0)",
    )
    .execute(&db)
    .await
    .unwrap();
    // Logged out, it stays as it was.
    let (_, page) = app.get("/s/oldsquad").await;
    assert!(
        page.contains("see and edit") && !page.contains("/join/"),
        "{page}"
    );
    let me = log_in_as(&app, GOOD_CODE, "/").await;
    let (_, page) = app.page_as(&me, "/s/oldsquad/settings").await;
    assert!(
        page.contains("Anyone with this link can join and edit"),
        "{page}"
    );
    assert!(page.contains("/invite/reset"), "{page}");
    let (visibility, role): (String, String) = sqlx::query_as(
        "SELECT s.visibility, m.role FROM squads s JOIN squad_members m ON m.squad_id = s.id",
    )
    .fetch_one(&db)
    .await
    .unwrap();
    assert_eq!((visibility.as_str(), role.as_str()), ("unlisted", "owner"));
    // The next one is too late: they can look, not edit.
    let friend = log_in_as(&app, FRIEND_CODE, "/").await;
    let (_, page) = app.page_as(&friend, "/s/oldsquad").await;
    assert!(page.contains("Ask a member for the invite link"), "{page}");
    let r = app.add("/s/oldsquad", "Aldric-Thornwood", "").await;
    assert_eq!(r.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn the_owner_chooses_who_can_see_a_squad() {
    let app = app_with_login(None).await;
    let owner = log_in_as(&app, GOOD_CODE, "/").await;
    let squad = app.squad_as(&owner, "Secret raiders").await;
    let invite = invite_path(&app.page_as(&owner, &format!("{squad}/settings")).await.1);
    let friend = log_in_as(&app, FRIEND_CODE, "/").await;
    app.post_as(&friend, &invite, &[]).await;
    let stranger = log_in_as(&app, STRANGER_CODE, "/").await;

    let settings = format!("{squad}/settings");
    let (_, page) = app.page_as(&owner, &settings).await;
    assert!(page.contains("Who can see it"), "{page}");
    assert!(
        page.contains("value=\"unlisted\" checked") && page.contains("value=\"private\""),
        "{page}"
    );
    // Only the owner chooses.
    let (_, page) = app.page_as(&friend, &settings).await;
    assert!(!page.contains("Who can see it"), "{page}");
    let to = |v: &'static str| [("visibility", v)];
    let r = app
        .post_as(&friend, &format!("{squad}/visibility"), &to("private"))
        .await;
    assert_eq!(r.status(), StatusCode::FORBIDDEN);

    let r = app
        .post_as(&owner, &format!("{squad}/visibility"), &to("private"))
        .await;
    assert_eq!(r.status(), StatusCode::SEE_OTHER);
    assert_eq!(r.headers()[header::LOCATION], settings.as_str());
    for who in ["", stranger.as_str()] {
        for path in [&squad, &settings] {
            let (status, page) = app.page_as(who, path).await;
            assert_eq!(status, StatusCode::NOT_FOUND);
            assert!(!page.contains("Secret raiders"), "{page}");
        }
    }
    // Members still see it; previews never name it, and there's no view link to pass on.
    let (status, page) = app.page_as(&friend, &squad).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        page.contains("<meta property=\"og:title\" content=\"zesquad\">"),
        "{page}"
    );
    assert!(page.contains("<title>zesquad</title>"), "{page}");
    assert!(!page.contains("id=\"squad-link\""), "{page}");
    let (_, page) = app.page_as(&friend, &settings).await;
    assert!(page.contains("<title>zesquad</title>"), "{page}");
    assert!(page.contains("id=\"invite-link\""), "{page}");

    // Back to old squads' "open" isn't a choice, nor is anything made up.
    for bad in ["open", "everyone", ""] {
        let r = app
            .post_as(&owner, &format!("{squad}/visibility"), &to(bad))
            .await;
        assert_eq!(r.status(), StatusCode::SEE_OTHER);
        assert_eq!(
            app.page_as("", &squad).await.0,
            StatusCode::NOT_FOUND,
            "{bad}"
        );
    }
    let r = app
        .post_as(&owner, &format!("{squad}/visibility"), &to("public"))
        .await;
    assert_eq!(r.status(), StatusCode::SEE_OTHER);
    let (status, page) = app.page_as(&stranger, &squad).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        page.contains("og:title\" content=\"Secret raiders\""),
        "{page}"
    );
}

#[tokio::test]
async fn the_front_page_lists_your_squads_first() {
    let app = app_with_login(None).await;
    let owner = log_in_as(&app, GOOD_CODE, "/").await;
    let friend = log_in_as(&app, FRIEND_CODE, "/").await;
    let raiders = app.squad_as(&owner, "Thursday raiders").await;
    let alts = app.squad_as(&friend, "Friendly alts").await;
    let secret = app.squad_as(&friend, "Friendly secrets").await;
    let invite = invite_path(&app.page_as(&owner, &format!("{raiders}/settings")).await.1);
    app.post_as(&friend, &invite, &[]).await;

    // The friend is in two squads, one theirs, and their front page says so.
    let (_, front) = app.page_as(&friend, "/").await;
    let section = front.split("<section class=\"intro\">").next().unwrap();
    assert!(section.contains("Thursday raiders"), "{section}");
    assert!(section.contains("yours · 0 characters"), "{section}");
    assert!(!section.contains("Forget"), "{section}");

    // The owner opened the friend's squads in this browser; only the one they can still see
    // stays, after their own.
    app.post_as(
        &friend,
        &format!("{secret}/visibility"),
        &[("visibility", "private")],
    )
    .await;
    let ids = |paths: &[&String]| {
        paths
            .iter()
            .map(|p| p.strip_prefix("/s/").unwrap())
            .collect::<Vec<_>>()
            .join(".")
    };
    let cookie = format!(
        "{owner}; zesquad_recent={}",
        ids(&[&secret, &alts, &raiders])
    );
    let (_, front) = app.page_as(&cookie, "/").await;
    let section = front.split("<section class=\"intro\">").next().unwrap();
    let raiders_at = section.find("Thursday raiders").unwrap();
    let alts_at = section.find("Friendly alts").unwrap();
    assert!(raiders_at < alts_at, "{section}");
    assert!(section.contains("Also opened in this browser"), "{section}");
    assert!(!section.contains("Friendly secrets"), "{section}");
    // Logged out, the same browser sees only what anyone with the links may.
    let (_, front) = app
        .page_as(
            &format!("zesquad_recent={}", ids(&[&secret, &alts, &raiders])),
            "/",
        )
        .await;
    let section = front.split("<section class=\"intro\">").next().unwrap();
    assert!(
        section.contains("Friendly alts") && section.contains("Thursday raiders"),
        "{section}"
    );
    assert!(!section.contains("Friendly secrets"), "{section}");
}

/// The id of the user with this BattleTag.
async fn user_id(db: &sqlx::SqlitePool, battletag: &str) -> i64 {
    sqlx::query_scalar("SELECT id FROM users WHERE battletag = ?")
        .bind(battletag)
        .fetch_one(db)
        .await
        .unwrap()
}

#[tokio::test]
async fn members_see_each_other_and_the_owner_manages_them() {
    let app = app_with_login(None).await;
    let owner = log_in_as(&app, GOOD_CODE, "/").await;
    let friend = log_in_as(&app, FRIEND_CODE, "/").await;
    let stranger = log_in_as(&app, STRANGER_CODE, "/").await;
    let squad = app.squad_as(&owner, "Thursday raiders").await;
    let invite = invite_path(&app.page_as(&owner, &format!("{squad}/settings")).await.1);
    app.post_as(&friend, &invite, &[]).await;
    app.post_as(&stranger, &invite, &[]).await;
    let db = app.db().await;
    let friend_id = user_id(&db, "Friendly#5678").await;
    let owner_id = user_id(&db, "Testhero#1234").await;

    let settings = format!("{squad}/settings");
    let (_, page) = app.page_as(&friend, &squad).await;
    assert!(!page.contains("Members ("), "{page}");
    let (_, page) = app.page_as(&friend, &settings).await;
    assert!(page.contains("Members (3)"), "{page}");
    assert!(
        page.contains("<strong>Testhero#1234</strong> · owner"),
        "{page}"
    );
    assert!(
        page.contains("<strong>Friendly#5678</strong> · you"),
        "{page}"
    );
    assert!(
        page.contains("/leave") && !page.contains("/members/"),
        "{page}"
    );
    assert!(!page.contains("Delete squad"), "{page}");
    // Anyone else never sees BattleTags.
    let (_, page) = app.page_as("", &squad).await;
    assert!(
        !page.contains("Testhero#1234") && !page.contains("Members ("),
        "{page}"
    );

    // A member can't remove anyone; the owner can't be removed.
    let remove = |id: i64| format!("{squad}/members/{id}/delete");
    let r = app.post_as(&friend, &remove(owner_id), &[]).await;
    assert_eq!(r.status(), StatusCode::FORBIDDEN);
    app.post_as(&owner, &remove(owner_id), &[]).await;
    // The owner removes the friend, who can then only look, and can't come back with the
    // invite link they saw as a member: it changed.
    let r = app.post_as(&owner, &remove(friend_id), &[]).await;
    assert_eq!(r.status(), StatusCode::SEE_OTHER);
    let (_, page) = app.page_as(&friend, &squad).await;
    assert!(page.contains("Ask a member for the invite link"), "{page}");
    assert_eq!(
        app.page_as(&friend, &settings).await.0,
        StatusCode::FORBIDDEN
    );
    let r = app.post_as(&friend, &invite, &[]).await;
    assert_eq!(r.status(), StatusCode::NOT_FOUND);
    let (_, page) = app.page_as(&owner, &settings).await;
    assert_ne!(invite_path(&page), invite);
    assert!(
        page.contains("Members (2)") && !page.contains("Friendly#5678"),
        "{page}"
    );

    // The stranger leaves (the invite link stays as it is); the owner can't.
    let invite = invite_path(&app.page_as(&owner, &format!("{squad}/settings")).await.1);
    let r = app.post_as(&stranger, &format!("{squad}/leave"), &[]).await;
    assert_eq!(r.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        invite_path(&app.page_as(&owner, &format!("{squad}/settings")).await.1),
        invite
    );
    let r = app.post_as(&owner, &format!("{squad}/leave"), &[]).await;
    assert_eq!(r.status(), StatusCode::FORBIDDEN);
    let roles: Vec<String> = sqlx::query_scalar("SELECT role FROM squad_members")
        .fetch_all(&db)
        .await
        .unwrap();
    assert_eq!(roles, ["owner"]);
}

#[tokio::test]
async fn the_owner_can_delete_a_squad() {
    let app = app_with_login(None).await;
    let owner = log_in_as(&app, GOOD_CODE, "/").await;
    let friend = log_in_as(&app, FRIEND_CODE, "/").await;
    let squad = app.squad_as(&owner, "Thursday raiders").await;
    let invite = invite_path(&app.page_as(&owner, &format!("{squad}/settings")).await.1);
    app.post_as(&friend, &invite, &[]).await;
    app.post_as(
        &friend,
        &format!("{squad}/characters"),
        &[("character", "Bryn-Ashvale"), ("tags", "raid")],
    )
    .await;
    let r = app.post_as(&friend, &format!("{squad}/delete"), &[]).await;
    assert_eq!(r.status(), StatusCode::FORBIDDEN);
    let (_, page) = app.page_as(&friend, &format!("{squad}/settings")).await;
    assert!(!page.contains("Delete squad"), "{page}");
    let (_, page) = app.page_as(&owner, &format!("{squad}/settings")).await;
    assert!(page.contains("Delete squad"), "{page}");
    let r = app.post_as(&owner, &format!("{squad}/delete"), &[]).await;
    assert_eq!(r.status(), StatusCode::SEE_OTHER);
    assert_eq!(r.headers()[header::LOCATION], "/");
    assert_eq!(app.page_as(&owner, &squad).await.0, StatusCode::NOT_FOUND);
    let db = app.db().await;
    let left: i64 = sqlx::query_scalar(
        "SELECT (SELECT COUNT(*) FROM characters) + (SELECT COUNT(*) FROM character_tags)
              + (SELECT COUNT(*) FROM squad_members) + (SELECT COUNT(*) FROM squads)",
    )
    .fetch_one(&db)
    .await
    .unwrap();
    assert_eq!(left, 0);
}

#[tokio::test]
async fn deleting_your_account_hands_your_squads_on() {
    let app = app_with_login(None).await;
    let owner = log_in_as(&app, GOOD_CODE, "/").await;
    let friend = log_in_as(&app, FRIEND_CODE, "/").await;
    let stranger = log_in_as(&app, STRANGER_CODE, "/").await;
    let shared = app.squad_as(&owner, "Thursday raiders").await;
    let alone = app.squad_as(&owner, "Solo alts").await;
    let invite = invite_path(&app.page_as(&owner, &format!("{shared}/settings")).await.1);
    // The friend joined first, so the squad goes to them, not the stranger.
    app.post_as(&friend, &invite, &[]).await;
    app.post_as(&stranger, &invite, &[]).await;
    let db = app.db().await;
    sqlx::query("UPDATE squad_members SET joined_at = joined_at - 10 WHERE user_id = ?")
        .bind(user_id(&db, "Friendly#5678").await)
        .execute(&db)
        .await
        .unwrap();
    // The friend's own squad isn't touched.
    let theirs = app.squad_as(&friend, "Friendly alts").await;

    let (_, page) = app.page_as(&owner, "/account").await;
    assert!(
        page.contains("Thursday raiders</a>: goes to <strong>Friendly#5678</strong>"),
        "{page}"
    );
    assert!(
        page.contains("Solo alts</a>: <strong>deleted</strong>"),
        "{page}"
    );
    assert!(!page.contains("Friendly alts"), "{page}");

    let r = app.post_as(&owner, "/account/delete", &[]).await;
    assert_eq!(r.status(), StatusCode::SEE_OTHER);
    assert_eq!(app.page_as(&friend, &alone).await.0, StatusCode::NOT_FOUND);
    let (_, page) = app.page_as(&friend, &format!("{shared}/settings")).await;
    assert!(
        page.contains("<strong>Friendly#5678</strong> · owner · you"),
        "{page}"
    );
    assert!(
        page.contains("Members (2)") && !page.contains("Testhero"),
        "{page}"
    );
    let (_, page) = app.page_as(&friend, &format!("{theirs}/settings")).await;
    assert!(page.contains("Members (1)"), "{page}");
}

#[tokio::test]
async fn public_squads_are_listed_for_everyone() {
    let app = app_with_login(None).await;
    let owner = log_in_as(&app, GOOD_CODE, "/").await;
    let (_, front) = app.get("/").await;
    assert!(front.contains("href=\"/squads\""), "{front}");
    let (status, page) = app.get("/squads").await;
    assert_eq!(status, StatusCode::OK);
    assert!(page.contains("No public squads yet"), "{page}");

    let listed = app.squad_as(&owner, "zebra raiders").await;
    let unlisted = app.squad_as(&owner, "Quiet alts").await;
    let first = app.squad_as(&owner, "Alpha team").await;
    for squad in [&listed, &first] {
        app.post_as(
            &owner,
            &format!("{squad}/visibility"),
            &[("visibility", "public")],
        )
        .await;
    }
    app.post_as(
        &owner,
        &format!("{first}/characters"),
        &[("character", "Aldric-Thornwood")],
    )
    .await;
    let (_, page) = app.get("/squads").await;
    let alpha = page.find("Alpha team").unwrap();
    let zebra = page.find("zebra raiders").unwrap();
    assert!(alpha < zebra, "{page}");
    assert!(page.contains("1 character · Europe"), "{page}");
    assert!(page.contains(&format!("href=\"{listed}\"")), "{page}");
    assert!(
        !page.contains("Quiet alts") && !page.contains(&unlisted),
        "{page}"
    );
}

#[tokio::test]
async fn without_accounts_there_is_no_public_list() {
    let app = app(false).await;
    let (_, front) = app.get("/").await;
    assert!(!front.contains("href=\"/squads\""), "{front}");
    assert_eq!(app.get("/squads").await.0, StatusCode::NOT_FOUND);
}

/// Logins, new squads and changes allowed per client in tests of the limits.
fn tight_limits(settings: &mut Settings) {
    let rule = |max| zesquad_core::limit::Rule { max, window: 3600 };
    settings.limits = zesquad::limits::Limits {
        login: zesquad_core::limit::Rule { max: 2, window: 60 },
        new_squad: rule(1),
        change: rule(3),
        new_guild: rule(2),
    };
}

#[tokio::test]
async fn repeated_logins_and_changes_are_slowed_down() {
    let app = app_tweaked(true, None, false, true, tight_limits).await;
    let me = log_in_bnet(&app, "/").await;
    // Logging in: the second start is the last.
    start_login(&app, "/").await;
    let r = app.get_with("/auth/login", "").await;
    assert_eq!(r.status(), StatusCode::TOO_MANY_REQUESTS);
    assert!(
        r.headers()[header::RETRY_AFTER]
            .to_str()
            .unwrap()
            .parse::<u64>()
            .unwrap()
            > 0
    );
    assert!(r.text().await.unwrap().contains("Wait a minute,"));

    // One new squad, then no more.
    let squad = app.squad_as(&me, "Thursday raiders").await;
    let r = app.post_as(&me, "/squads", &[("name", "Another")]).await;
    assert_eq!(r.status(), StatusCode::TOO_MANY_REQUESTS);
    // The wait is as long as the limit says.
    assert!(r.text().await.unwrap().contains("Wait an hour,"));

    // Changes: three, then refused, htmx included; looking is never limited.
    for tag in ["a", "b", "c"] {
        let r = app
            .post_as(
                &me,
                &format!("{squad}/characters"),
                &[("character", &format!("Aldric-Thorn{tag}"))],
            )
            .await;
        assert_eq!(r.status(), StatusCode::SEE_OTHER);
    }
    let r = app
        .htmx_as(
            &me,
            &format!("{squad}/characters"),
            &[("character", "Bryn-Ashvale")],
        )
        .await;
    assert_eq!(r.status(), StatusCode::TOO_MANY_REQUESTS);
    for _ in 0..10 {
        assert_eq!(app.page_as(&me, &squad).await.0, StatusCode::OK);
    }
    // Logging out always works.
    let r = app.post_as(&me, "/logout", &[]).await;
    assert_eq!(r.status(), StatusCode::SEE_OTHER);
    // The page tells htmx users why nothing happened.
    let (_, page) = app.get("/").await;
    assert!(page.contains("/slow-down.js"), "{page}");
}

#[tokio::test]
async fn behind_a_trusted_proxy_each_client_has_its_own_limits() {
    use zesquad::limits::TrustProxy;
    /// Starts a squad with these headers; the status.
    async fn post(app: &App, headers: &[(&str, &str)]) -> StatusCode {
        let mut request = app
            .http
            .post(format!("{}/squads", app.base))
            .form(&[("name", "Raiders")]);
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        request.send().await.unwrap().status()
    }
    let app_trusting = |trust| {
        app_tweaked(false, None, false, false, move |s| {
            tight_limits(s);
            s.trust_proxy = trust;
        })
    };
    let (ok, slow) = (StatusCode::SEE_OTHER, StatusCode::TOO_MANY_REQUESTS);

    let app = app_trusting(TrustProxy::Cloudflare).await;
    assert_eq!(post(&app, &[("cf-connecting-ip", "192.0.2.1")]).await, ok);
    assert_eq!(post(&app, &[("cf-connecting-ip", "192.0.2.1")]).await, slow);
    assert_eq!(post(&app, &[("cf-connecting-ip", "192.0.2.2")]).await, ok);
    // An X-Forwarded-For a client made up means nothing here.
    assert_eq!(
        post(
            &app,
            &[
                ("cf-connecting-ip", "192.0.2.2"),
                ("x-forwarded-for", "192.0.2.9")
            ]
        )
        .await,
        slow
    );

    // Behind Caddy or nginx, a made-up CF-Connecting-IP means nothing; nor does what the
    // client put in front of the proxy's X-Forwarded-For entry.
    let app = app_trusting(TrustProxy::ForwardedFor).await;
    assert_eq!(
        post(&app, &[("x-forwarded-for", "203.0.113.1, 192.0.2.1")]).await,
        ok
    );
    assert_eq!(
        post(
            &app,
            &[
                ("x-forwarded-for", "203.0.113.2, 192.0.2.1"),
                ("cf-connecting-ip", "192.0.2.3")
            ]
        )
        .await,
        slow
    );
    assert_eq!(post(&app, &[("x-forwarded-for", "192.0.2.2")]).await, ok);

    // Not trusted: every header is ignored, and everyone here is 127.0.0.1.
    let app = app_trusting(TrustProxy::None).await;
    assert_eq!(post(&app, &[("cf-connecting-ip", "192.0.2.1")]).await, ok);
    assert_eq!(post(&app, &[("cf-connecting-ip", "192.0.2.2")]).await, slow);
}

// ---- guilds ----

/// Rows in the guild page's roster table.
fn roster_rows(page: &str) -> usize {
    page.split("<tbody>").nth(1).map_or(0, |t| {
        t.split("</tbody>").next().unwrap().matches("<tr").count()
    })
}

#[tokio::test]
async fn a_guild_page_looks_the_guild_up_and_lists_its_members() {
    let app = app(true).await;
    let path = format!("/g/eu/{GUILD}");
    let (status, first) = app.get(&path).await;
    assert_eq!(status, StatusCode::OK);
    if !first.contains("Brannoc") {
        assert!(first.contains("Looking up the guild"), "{first}");
        assert!(first.contains("hx-trigger=\"every 3s\""), "{first}");
    }
    let page = app.wait_for(&path, "Brannoc").await;
    assert!(
        page.contains("<h1 class=\"squad-name\">Order of the Lantern</h1>"),
        "{page}"
    );
    assert!(page.contains("61 members · Alliance"), "{page}");
    assert!(
        page.contains("<title>Order of the Lantern · zesquad</title>"),
        "{page}"
    );
    assert!(!page.contains("every 3s"), "stops polling: {page}");
    // Guild master first, with the roster's class and race names.
    assert_eq!(roster_rows(&page), 50);
    let first_row = page.split("<tbody>").nth(1).unwrap();
    let first_row = first_row.split("</tr>").next().unwrap();
    assert!(first_row.contains("Aldric"), "{first_row}");
    assert!(first_row.contains("Guild Master"), "{first_row}");
    assert!(
        first_row.contains("Rogue") && first_row.contains("Night Elf"),
        "{first_row}"
    );
    assert!(page.contains("Page 1 of 2"), "{page}");
    assert!(
        page.contains(&format!("href=\"{path}?page=2\" rel=\"next\"")),
        "{page}"
    );
    let (_, second) = app.get(&format!("{path}?page=2")).await;
    assert_eq!(roster_rows(&second), 11);
    assert!(second.contains("rel=\"prev\""), "{second}");

    // Opening it again reads what's stored: the guild and roster were asked for once.
    app.get(&path).await;
    assert_eq!(app.fake.guild_lookups.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn a_guild_roster_can_be_searched_filtered_and_sorted() {
    let app = app(true).await;
    let path = format!("/g/eu/{GUILD}");
    app.wait_for(&path, "Brannoc").await;

    let (_, found) = app.get(&format!("{path}?q=BRANNOC")).await;
    assert_eq!(roster_rows(&found), 5, "{found}");
    assert!(found.contains("5 of 61 members"), "{found}");
    assert!(!found.contains("Elowen"), "{found}");
    assert!(
        found.contains("value=\"brannoc\""),
        "the search box keeps it: {found}"
    );
    assert!(!found.contains("Page 1 of"), "{found}");

    let (_, surname) = app.get(&format!("{path}?q=brann%20ash")).await;
    assert_eq!(roster_rows(&surname), 1, "{surname}");

    let (_, rogues) = app.get(&format!("{path}?class=rogue")).await;
    assert!(rogues.contains("class=\"chip coloured on\""), "{rogues}");
    let rows = roster_rows(&rogues);
    assert!(rows > 0 && rows < 61, "{rows}");
    assert_eq!(
        rogues.matches("<td class=\"class\">Rogue</td>").count(),
        rows,
        "{rogues}"
    );
    // Sorting keeps the class; the search box carries both.
    assert!(
        rogues.contains("href=\"/g/eu/order-of-the-lantern?class=rogue&#38;sort=level\""),
        "{rogues}"
    );
    assert!(
        rogues.contains("name=\"class\" value=\"rogue\""),
        "{rogues}"
    );

    let (_, by_level) = app.get(&format!("{path}?sort=level")).await;
    let top = by_level.split("<tbody>").nth(1).unwrap();
    let top = top.split("</tr>").next().unwrap();
    assert!(top.contains("<td class=\"num\">60</td>"), "{top}");

    let (_, nobody) = app.get(&format!("{path}?q=zzz&page=4")).await;
    assert!(nobody.contains("Nobody matches."), "{nobody}");
    assert!(nobody.contains("0 of 61 members"), "{nobody}");
}

#[tokio::test]
async fn unknown_guilds_are_404() {
    let app = app(true).await;
    let path = "/g/eu/no-such-guild";
    let page = app.wait_for(path, "Not on the Armory").await;
    assert!(page.contains("Not on the Armory"), "{page}");
    assert_eq!(app.get(path).await.0, StatusCode::NOT_FOUND);
    // The fake knows the guild in EU only.
    app.wait_for(&format!("/g/us/{GUILD}"), "Not on the Armory")
        .await;
    for bad in ["/g/xx/spark", "/g/eu/a--b", "/g/eu/a.b", "/g/eu/-x"] {
        assert_eq!(app.get(bad).await.0, StatusCode::NOT_FOUND, "{bad}");
    }
}

#[tokio::test]
async fn guild_pages_need_armory_lookups() {
    let app = app(false).await;
    assert_eq!(
        app.get(&format!("/g/eu/{GUILD}")).await.0,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn opening_many_new_guilds_is_slowed_down() {
    let app = app_tweaked(true, None, false, false, tight_limits).await;
    let known = format!("/g/eu/{GUILD}");
    assert_eq!(app.get(&known).await.0, StatusCode::OK);
    app.get("/g/eu/second-guild").await;
    let (status, body) = app.get("/g/eu/third-guild").await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{body}");
    // Guilds already followed don't count.
    app.wait_for(&known, "Brannoc").await;
    assert_eq!(app.get(&known).await.0, StatusCode::OK);
}

#[tokio::test]
async fn character_cards_link_to_their_guild() {
    let app = app(true).await;
    let squad = app.squad("Lanterns").await;
    app.add(&squad, "Aldric-Thornwood", "").await;
    let page = app.wait_for(&squad, "Order of the Lantern").await;
    assert!(
        page.contains(&format!(
            "<a href=\"/g/eu/{GUILD}\" class=\"guild\">&lt;Order of the Lantern&gt;</a> · item level 14"
        )),
        "{page}"
    );
    // The front page's made-up characters aren't linked anywhere.
    let (_, front) = app.get("/").await;
    assert!(front.contains("&lt;Lanterns of Ashwood&gt;"), "{front}");
    assert!(!front.contains("class=\"guild\""), "{front}");
}

#[tokio::test]
async fn without_lookups_guilds_are_not_linked() {
    let app = app(false).await;
    let squad = app.squad("Lanterns").await;
    app.add(&squad, "Aldric-Thornwood", "").await;
    let (_, page) = app.get(&squad).await;
    assert!(!page.contains("/g/eu/"), "{page}");
}
