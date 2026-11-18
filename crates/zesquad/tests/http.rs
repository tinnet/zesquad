//! Black-box tests: the real app on a random port with a temporary database, talking to a fake
//! Battle.net in the same process, driven over HTTP the way a browser (or htmx) would.

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use axum::Router;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use reqwest::redirect::Policy;
use tempfile::TempDir;
use zesquad::Settings;

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
}

async fn fake_token(headers: HeaderMap) -> Response {
    // base64("test-id:test-secret")
    let expected = "Basic dGVzdC1pZDp0ZXN0LXNlY3JldA==";
    if headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        != Some(expected)
    {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    axum::Json(serde_json::json!({
        "access_token": TOKEN, "token_type": "bearer", "expires_in": 86399
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
    if q.get("namespace").map(String::as_str) != Some("profile-forever-eu") {
        return StatusCode::FORBIDDEN.into_response();
    }
    let not_found = (
        StatusCode::NOT_FOUND,
        r#"{"code":404,"type":"BLZWEBAPI00000404","detail":"Not Found"}"#,
    )
        .into_response();
    let (key, endpoint) = match segments {
        [key] => (*key, "summary"),
        [key, endpoint] => (*key, *endpoint),
        _ => return not_found,
    };
    let fixture = match endpoint {
        "summary" => "summary",
        "character-media" => "media",
        "equipment" => "equipment",
        "specializations" => "specializations",
        "professions" => "professions",
        _ => return not_found, // hunter-pets: the fixture character is a rogue
    };
    if key != KNOWN {
        return not_found;
    }
    let path = format!(
        "{}/../zesquad-core/fixtures/{fixture}.json",
        env!("CARGO_MANIFEST_DIR")
    );
    (
        [(header::CONTENT_TYPE, "application/json")],
        std::fs::read_to_string(path).unwrap(),
    )
        .into_response()
}

async fn start_fake() -> (String, Fake) {
    let fake = Fake::default();
    let app = Router::new()
        .route("/token", post(fake_token))
        .route("/profile/wow/character/{*rest}", get(fake_character_path))
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

// ---- the app ----

struct App {
    base: String,
    http: reqwest::Client,
    fake: Fake,
    _dir: TempDir,
}

async fn app(with_armory: bool) -> App {
    app_with(with_armory, None).await
}

async fn app_with(with_armory: bool, password: Option<&str>) -> App {
    let dir = tempfile::tempdir().unwrap();
    let (fake_url, fake) = start_fake().await;
    let armory = with_armory.then(|| {
        zesquad::armory::Config::new(
            CLIENT_ID.into(),
            CLIENT_SECRET.into(),
            "eu".into(),
            Some(&fake_url),
            &format!("{fake_url}/token"),
        )
        .unwrap()
    });
    let settings = Settings {
        listen: SocketAddr::from(([127, 0, 0, 1], 0)),
        database: dir.path().join("zesquad.db"),
        refresh: Duration::from_secs(3600),
        region: "eu".into(),
        armory,
        password: password.map(str::to_owned),
    };
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
        _dir: dir,
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

    /// Creates a squad and returns its path, `/s/<uuid>`.
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
    assert!(uuid_like(id), "{id}");
    let (status, page) = app.get(&squad).await;
    assert_eq!(status, StatusCode::OK);
    assert!(page.contains("value=\"Thursday raiders\""), "{page}");
    assert!(page.contains("No characters yet"));
}

fn uuid_like(s: &str) -> bool {
    s.len() == 36 && s.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
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

#[tokio::test]
async fn renaming_a_squad() {
    let app = app(false).await;
    let squad = app.squad("Old name").await;
    let r = app
        .post(&format!("{squad}/name"), &[("name", "New name")])
        .await;
    assert_eq!(r.status(), StatusCode::SEE_OTHER);
    assert!(app.get(&squad).await.1.contains("value=\"New name\""));
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
        "&#60;Order of the Lantern&#62; · item level 14 · Combat 17 / Assassination 5",
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
