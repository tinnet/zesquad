//! Pages and form handlers. Every form works as a plain POST with a redirect; with htmx the
//! squad page swaps just the roster instead of reloading.

use std::sync::Arc;

use askama::Template;
use askama_web::WebTemplate;
use axum::extract::{Form, FromRequestParts, Path, Query, RawPathParams, State};
use axum::http::request::Parts;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{AppendHeaders, IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use sqlx::SqlitePool;
use tokio::sync::Notify;
use zesquad_core::armory::Profile;
use zesquad_core::auctions::{self, Money};
use zesquad_core::recent::Recent;
use zesquad_core::refresh::Status;
use zesquad_core::sparkline::{Sparkline, sparkline};
use zesquad_core::tags::{self, CharacterTag, Colour};
use zesquad_core::{CharacterName, Region, SquadName, Tag, parse_tags};

use crate::db;
use crate::version;

#[derive(Clone)]
pub struct AppState {
    pub db: SqlitePool,
    pub wake: Arc<Notify>,
    /// The region the new-squad form starts on.
    pub default_region: Region,
    pub armory_enabled: bool,
    /// The auction sync runs.
    pub auctions_enabled: bool,
    pub armory_cache: Arc<db::ArmoryCache>,
    /// Set when a password protects the app.
    pub auth: Option<Arc<crate::auth::Auth>>,
}

/// The open routes (health check, version, static files, logging in and out) and, behind the
/// password when one is set, everything else, including unknown paths.
pub fn router(state: AppState) -> Router {
    let open = Router::new()
        .route("/healthz", get(healthz))
        .route(
            "/version",
            get(|| async { Json(serde_json::json!({ "version": version::short() })) }),
        )
        .route("/static/{version}/{*path}", get(crate::assets::serve))
        .route("/login", get(login_page).post(login))
        // Logging out needs no session: it only clears the cookie.
        .route("/logout", post(logout));
    let protected = Router::new()
        .route("/", get(index))
        .route("/auctions", get(auctions_page))
        .route("/squads", post(create_squad))
        .route("/recent/{squad}/forget", post(forget_squad))
        .route("/s/{squad}", get(squad_page))
        .route("/s/{squad}/roster", get(roster_partial))
        .route("/s/{squad}/name", post(rename))
        .route("/s/{squad}/characters", post(add_character))
        .route("/s/{squad}/characters/{id}/tags", post(add_tags))
        .route(
            "/s/{squad}/characters/{id}/tags/{tag}/delete",
            post(remove_tag),
        )
        .route("/s/{squad}/tags/{tag}/colour", post(set_colour))
        .route("/s/{squad}/characters/{id}/delete", post(remove_character))
        .route("/s/{squad}/characters/{id}/refresh", post(refresh))
        .fallback(|| async { AppError::NotFound })
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            crate::auth::require,
        ));
    open.merge(protected).with_state(state)
}

/// A squad from the `{squad}` part of the path; unknown ids are a 404.
impl FromRequestParts<AppState> for db::Squad {
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self> {
        let params = RawPathParams::from_request_parts(parts, state)
            .await
            .map_err(|_| AppError::NotFound)?;
        let id = params
            .iter()
            .find_map(|(name, value)| (name == "squad").then_some(value))
            .ok_or(AppError::NotFound)?;
        db::squad(&state.db, id).await?.ok_or(AppError::NotFound)
    }
}

/// Whether htmx sent the request (it swaps part of the page rather than loading a new one).
pub struct Htmx(pub bool);

impl Htmx {
    #[must_use]
    pub fn of(headers: &HeaderMap) -> bool {
        headers.contains_key("hx-request")
    }
}

impl<S: Send + Sync> FromRequestParts<S> for Htmx {
    type Rejection = std::convert::Infallible;

    fn from_request_parts(
        parts: &mut Parts,
        _: &S,
    ) -> impl Future<Output = std::result::Result<Self, Self::Rejection>> + Send {
        std::future::ready(Ok(Self(Self::of(&parts.headers))))
    }
}

/// `scheme://host` the request was sent to, as the browser sees it (through a proxy too);
/// `None` when the host doesn't look like one.
pub struct Origin(Option<String>);

impl Origin {
    fn of(headers: &HeaderMap) -> Self {
        let host = ["x-forwarded-host", header::HOST.as_str()]
            .iter()
            .find_map(|name| headers.get(*name)?.to_str().ok());
        let https = !crate::auth::secure(headers).is_empty();
        Self(host.and_then(|host| zesquad_core::link::origin(https, host)))
    }

    /// The full link to `path`; just the path without an origin.
    fn link(&self, path: &str) -> String {
        format!("{}{path}", self.0.as_deref().unwrap_or_default())
    }
}

impl<S: Send + Sync> FromRequestParts<S> for Origin {
    type Rejection = std::convert::Infallible;

    fn from_request_parts(
        parts: &mut Parts,
        _: &S,
    ) -> impl Future<Output = std::result::Result<Self, Self::Rejection>> + Send {
        std::future::ready(Ok(Self::of(&parts.headers)))
    }
}

/// A squad's page; every form on it posts below this.
#[must_use]
pub fn squad_path(id: &str) -> String {
    format!("/s/{id}")
}

// ---- errors ----

pub enum AppError {
    NotFound,
    Internal(anyhow::Error),
}

impl<E: Into<anyhow::Error>> From<E> for AppError {
    fn from(e: E) -> Self {
        Self::Internal(e.into())
    }
}

#[derive(Template, WebTemplate)]
#[template(path = "error.html")]
struct ErrorPage {
    /// Show the log-out button (a password is set).
    logout: bool,
    title: &'static str,
    message: &'static str,
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, title, message) = match self {
            Self::NotFound => (
                StatusCode::NOT_FOUND,
                "Not found",
                "There's nothing here. If someone sent you a squad link, check it's complete.",
            ),
            Self::Internal(e) => {
                tracing::error!("request failed: {e:#}");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Something went wrong",
                    "That didn't work. Try again in a moment.",
                )
            }
        };
        let page = ErrorPage {
            logout: false,
            title,
            message,
        };
        (status, page).into_response()
    }
}

type Result<T> = std::result::Result<T, AppError>;

// ---- views ----

pub struct MemberView {
    pub id: i64,
    pub name: String,
    pub surname: String,
    /// From the Armory first, then the typed ones.
    pub tags: Vec<Chip>,
    pub status: Status,
    pub profile: Arc<Profile>,
    /// The Armory answered at some point; still shown while a refresh is pending or failed.
    pub has_profile: bool,
    pub class_key: String,
    pub last_login: Option<String>,
    pub checked: Option<String>,
}

impl MemberView {
    fn new(m: db::Member, now: i64, colours: &Colours) -> Self {
        let profile = m.armory.profile;
        let tags = tags::merge(tags::auto_tags(&profile), m.tags)
            .into_iter()
            .map(|tag| Chip {
                paint: colours.of(&tag.key),
                tag,
            })
            .collect();
        Self {
            has_profile: m.armory.answered,
            id: m.id,
            class_key: profile.class_key(),
            last_login: profile.last_login_ms.map(|ms| ago(now - ms / 1000)),
            checked: m.fetched_at.map(|t| ago(now - t)),
            name: profile.name.clone().unwrap_or(m.name),
            surname: profile.surname.clone().unwrap_or(m.surname),
            profile,
            tags,
            status: m.status,
        }
    }

    /// Guild, item level and talents, separated by dots; empty parts left out.
    #[must_use]
    pub fn details_line(&self) -> String {
        let p = &self.profile;
        let talents = p
            .talents
            .iter()
            .map(|(tree, points)| format!("{tree} {points}"))
            .collect::<Vec<_>>()
            .join(" / ");
        [
            p.guild.as_ref().map(|g| format!("<{g}>")),
            p.item_level.map(|l| format!("item level {l}")),
            Some(talents).filter(|t| !t.is_empty()),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" · ")
    }

    #[must_use]
    pub fn has_details(&self) -> bool {
        !(self.profile.gear.is_empty()
            && self.profile.professions.is_empty()
            && self.profile.pets.is_empty())
    }
}

/// A tag's colour, if it has one, and how a chip shows it.
pub struct Paint(Option<Colour>);

impl Paint {
    #[must_use]
    pub fn is_set(&self) -> bool {
        self.0.is_some()
    }

    /// Extra classes for the chip.
    #[must_use]
    pub fn class(&self) -> &'static str {
        if self.is_set() { " coloured" } else { "" }
    }

    /// CSS variables for the chip; empty without a colour.
    #[must_use]
    pub fn style(&self) -> String {
        self.0.as_ref().map_or_else(String::new, |c| {
            format!("--tag: {c}; --tag-text: {}", c.text())
        })
    }

    /// The colour input's value: grey for a tag without a colour.
    #[must_use]
    pub fn picker_value(&self) -> &str {
        self.0.as_ref().map_or("#888888", Colour::as_str)
    }
}

/// A tag as shown on a character, coloured.
pub struct Chip {
    pub tag: CharacterTag,
    pub paint: Paint,
}

/// A tag used in the squad, for the filter and the colour picker.
pub struct SquadTag {
    pub label: String,
    pub key: String,
    pub paint: Paint,
    /// Someone picked the colour (it isn't the default).
    pub picked: bool,
}

/// The squad's picked tag colours, falling back to the defaults.
struct Colours(std::collections::HashMap<String, Colour>);

impl Colours {
    fn of(&self, key: &str) -> Paint {
        Paint(
            self.0
                .get(key)
                .cloned()
                .or_else(|| tags::default_colour(key)),
        )
    }
}

/// "3 hours ago": precise enough for "when was this character last played".
fn ago(secs: i64) -> String {
    let (n, unit) = match secs {
        ..60 => return "just now".to_owned(),
        60..3600 => (secs / 60, "minute"),
        3600..86_400 => (secs / 3600, "hour"),
        _ => (secs / 86_400, "day"),
    };
    format!("{n} {unit}{} ago", if n == 1 { "" } else { "s" })
}

/// The roster section: add form, tag filter and members. Its own template so htmx can swap it.
pub struct Roster {
    pub squad_id: String,
    pub members: Vec<MemberView>,
    /// Characters in the squad, before the tag filter.
    pub total: usize,
    pub tags: Vec<SquadTag>,
    /// Key of the tag being filtered on.
    pub filter: Option<String>,
    pub armory_enabled: bool,
    pub form: Outcome,
}

impl Roster {
    /// The squad's URL, which every form posts below.
    #[must_use]
    pub fn base(&self) -> String {
        squad_path(&self.squad_id)
    }

    /// While anything is waiting for its first fetch, the page polls for the result.
    #[must_use]
    pub fn polling(&self) -> bool {
        self.armory_enabled && self.members.iter().any(|m| m.status == Status::Pending)
    }

    #[must_use]
    pub fn filter_query(&self) -> String {
        filter_query(self.filter.as_deref())
    }

    #[must_use]
    pub fn filter_value(&self) -> &str {
        self.filter.as_deref().unwrap_or_default()
    }

    #[must_use]
    pub fn is_filter(&self, key: &str) -> bool {
        self.filter.as_deref() == Some(key)
    }
}

/// The tag filter from a query or form as a tag key: lower case, `None` when empty.
fn tag_filter(filter: Option<String>) -> Option<String> {
    filter.filter(|t| !t.is_empty()).map(|t| t.to_lowercase())
}

/// `?tag=<key>` for a tag filter, empty for none.
fn filter_query(filter: Option<&str>) -> String {
    filter.map_or_else(String::new, |t| format!("?tag={}", urlencode(t)))
}

/// Percent-encodes everything but unreserved characters, for tags in URLs: askama's
/// `urlencode_strict`, which the templates use as a filter.
#[must_use]
pub fn urlencode(s: &str) -> String {
    let Ok(encoded) = askama::filters::urlencode_strict(s);
    encoded.to_string()
}

#[derive(Template, WebTemplate)]
#[template(path = "index.html")]
struct IndexPage {
    /// Show the log-out button (a password is set).
    logout: bool,
    /// Squads this browser opened lately, newest first.
    recent: Vec<db::SquadSummary>,
    error: Option<String>,
    name_input: String,
    /// The region picked in the form.
    region: Region,
}

#[derive(Template, WebTemplate)]
#[template(path = "squad.html")]
struct SquadPage {
    /// Show the log-out button (a password is set).
    logout: bool,
    squad: db::Squad,
    /// The page's full link, to copy; just the path when the request's host looks wrong.
    link: String,
    roster: Roster,
}

impl SquadPage {
    /// One line about the squad for link previews: "6 characters in Europe on zesquad, …".
    fn summary(&self) -> String {
        let n = self.roster.total;
        format!(
            "{n} character{} in {} on zesquad, friend squads for WoW: Forever.",
            if n == 1 { "" } else { "s" },
            self.squad.region.name()
        )
    }
}

#[derive(Template, WebTemplate)]
#[template(path = "roster.html")]
struct RosterPartial {
    roster: Roster,
}

// ---- handlers ----

#[derive(Template, WebTemplate)]
#[template(path = "login.html")]
struct LoginPage {
    logout: bool,
    next: String,
    wrong: bool,
}

#[derive(Deserialize)]
struct LoginQuery {
    next: Option<String>,
}

async fn login_page(State(state): State<AppState>, Query(q): Query<LoginQuery>) -> Response {
    let next = zesquad_core::redirect::safe_next(q.next.as_deref()).to_owned();
    if state.auth.is_none() {
        return Redirect::to(&next).into_response();
    }
    LoginPage {
        logout: false,
        next,
        wrong: false,
    }
    .into_response()
}

#[derive(Deserialize)]
struct LoginForm {
    password: String,
    next: Option<String>,
}

async fn login(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<LoginForm>,
) -> Response {
    let next = zesquad_core::redirect::safe_next(form.next.as_deref()).to_owned();
    let Some(auth) = &state.auth else {
        return Redirect::to(&next).into_response();
    };
    if auth.password_matches(&form.password) {
        let cookie = auth.session_cookie(&headers);
        return ([(header::SET_COOKIE, cookie)], Redirect::to(&next)).into_response();
    }
    crate::auth::wrong_password_delay().await;
    let page = LoginPage {
        logout: false,
        next,
        wrong: true,
    };
    (StatusCode::UNAUTHORIZED, page).into_response()
}

async fn logout() -> Response {
    (
        [(header::SET_COOKIE, crate::auth::logout_cookie())],
        Redirect::to("/login"),
    )
        .into_response()
}

// ---- recently opened squads ----

const RECENT_COOKIE: &str = "zesquad_recent";

fn recent(headers: &HeaderMap) -> Recent {
    crate::auth::cookie(headers, RECENT_COOKIE).map_or_else(Recent::default, Recent::parse)
}

/// The `Set-Cookie` header that remembers `recent`; an empty list clears the cookie.
fn recent_cookie(headers: &HeaderMap, recent: &Recent) -> (header::HeaderName, String) {
    let max_age = if recent.is_empty() { 0 } else { 31_536_000 };
    (
        header::SET_COOKIE,
        format!(
            "{RECENT_COOKIE}={}; Path=/; HttpOnly; SameSite=Lax; Max-Age={max_age}{}",
            recent.to_cookie(),
            crate::auth::secure(headers)
        ),
    )
}

/// The squads this browser opened lately that still exist, and the cookie to write back when
/// some are gone.
async fn recent_squads(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<(Vec<db::SquadSummary>, Option<(header::HeaderName, String)>)> {
    let mut recent = recent(headers);
    if recent.is_empty() {
        return Ok((Vec::new(), None));
    }
    let squads = db::squad_summaries(&state.db, recent.ids()).await?;
    let mut cookie = None;
    if squads.len() < recent.ids().len() {
        recent.retain(|id| squads.iter().any(|s| s.id == id));
        cookie = Some(recent_cookie(headers, &recent));
    }
    Ok((squads, cookie))
}

async fn index(State(state): State<AppState>, headers: HeaderMap) -> Result<Response> {
    let (recent, cookie) = recent_squads(&state, &headers).await?;
    let page = IndexPage {
        logout: state.auth.is_some(),
        recent,
        error: None,
        name_input: String::new(),
        region: state.default_region,
    };
    Ok((AppendHeaders(cookie), page).into_response())
}

async fn forget_squad(headers: HeaderMap, Path(squad): Path<String>) -> Response {
    let mut recent = recent(&headers);
    recent.forget(&squad);
    ([recent_cookie(&headers, &recent)], Redirect::to("/")).into_response()
}

#[derive(Deserialize)]
struct NameForm {
    name: String,
}

#[derive(Deserialize)]
struct NewSquadForm {
    name: String,
    /// Missing means the default region.
    region: Option<String>,
}

async fn create_squad(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<NewSquadForm>,
) -> Result<Response> {
    let region = form
        .region
        .as_deref()
        .map_or(Ok(state.default_region), str::parse);
    let parsed = SquadName::new(&form.name)
        .map_err(|e| e.to_string())
        .and_then(|name| Ok((name, region.clone().map_err(|e| e.to_string())?)));
    match parsed {
        Ok((name, region)) => {
            let id = db::create_squad(&state.db, &name, region).await?;
            Ok(Redirect::to(&squad_path(&id)).into_response())
        }
        Err(error) => {
            let (recent, _) = recent_squads(&state, &headers).await?;
            let page = IndexPage {
                logout: state.auth.is_some(),
                recent,
                error: Some(error),
                name_input: form.name,
                region: region.unwrap_or(state.default_region),
            };
            Ok((StatusCode::UNPROCESSABLE_ENTITY, page).into_response())
        }
    }
}

#[derive(Deserialize)]
struct FilterQuery {
    tag: Option<String>,
}

/// The roster with `filter`, a tag key from `tag_filter`, applied.
async fn build_roster(
    state: &AppState,
    squad_id: &str,
    filter: Option<String>,
    form: Outcome,
) -> Result<Roster> {
    let now = db::now();
    let colours = Colours(db::tag_colours(&state.db, squad_id).await?);
    let mut members: Vec<MemberView> = db::members(&state.db, &state.armory_cache, squad_id)
        .await?
        .into_iter()
        .map(|m| MemberView::new(m, now, &colours))
        .collect();
    // Every tag in the squad, alphabetically, spelled the way it first appears.
    let mut labels = std::collections::BTreeMap::new();
    for chip in members.iter().flat_map(|m| &m.tags) {
        labels
            .entry(chip.tag.key.clone())
            .or_insert_with(|| chip.tag.label.clone());
    }
    let tags = labels
        .into_iter()
        .map(|(key, label)| SquadTag {
            paint: colours.of(&key),
            picked: colours.0.contains_key(&key),
            label,
            key,
        })
        .collect();
    let total = members.len();
    if let Some(key) = &filter {
        members.retain(|m| m.tags.iter().any(|c| &c.tag.key == key));
    }
    Ok(Roster {
        squad_id: squad_id.to_owned(),
        members,
        total,
        tags,
        filter,
        armory_enabled: state.armory_enabled,
        form,
    })
}

async fn squad_page(
    State(state): State<AppState>,
    headers: HeaderMap,
    squad: db::Squad,
    Query(q): Query<FilterQuery>,
) -> Result<Response> {
    let roster = build_roster(&state, &squad.id, tag_filter(q.tag), Outcome::default()).await?;
    let mut recent = recent(&headers);
    recent.visit(&squad.id);
    let origin = Origin::of(&headers);
    let page = SquadPage {
        logout: state.auth.is_some(),
        link: origin.link(&squad_path(&squad.id)),
        squad,
        roster,
    };
    Ok(([recent_cookie(&headers, &recent)], page).into_response())
}

async fn roster_partial(
    State(state): State<AppState>,
    squad: db::Squad,
    Query(q): Query<FilterQuery>,
) -> Result<RosterPartial> {
    Ok(RosterPartial {
        roster: build_roster(&state, &squad.id, tag_filter(q.tag), Outcome::default()).await?,
    })
}

async fn rename(
    State(state): State<AppState>,
    squad: db::Squad,
    Form(form): Form<NameForm>,
) -> Result<Redirect> {
    // An invalid name just keeps the old one; the field is required in the form anyway.
    if let Ok(name) = SquadName::new(&form.name) {
        db::rename_squad(&state.db, &squad.id, &name).await?;
    }
    Ok(Redirect::to(&squad_path(&squad.id)))
}

/// The state of the roster's forms after a change. An error (bad input) is shown in the
/// roster, keeping what was typed.
#[derive(Default)]
pub struct Outcome {
    pub error: Option<String>,
    pub character_input: String,
    pub tags_input: String,
    /// Keep the tag colours open after picking one.
    pub colours_open: bool,
}

impl Outcome {
    fn error(message: String) -> Self {
        Self {
            error: Some(message),
            ..Self::default()
        }
    }
}

/// After a change: htmx gets the new roster, a plain form post a redirect back to the page.
async fn respond(
    state: &AppState,
    Htmx(htmx): Htmx,
    origin: &Origin,
    squad: db::Squad,
    filter: Option<String>,
    outcome: Outcome,
) -> Result<Response> {
    let filter = tag_filter(filter);
    let failed = outcome.error.is_some();
    if !htmx && !failed {
        let query = filter_query(filter.as_deref());
        return Ok(Redirect::to(&format!("{}{query}", squad_path(&squad.id))).into_response());
    }
    let roster = build_roster(state, &squad.id, filter, outcome).await?;
    if htmx {
        return Ok(RosterPartial { roster }.into_response());
    }
    let status = if failed {
        StatusCode::UNPROCESSABLE_ENTITY
    } else {
        StatusCode::OK
    };
    let page = SquadPage {
        logout: state.auth.is_some(),
        link: origin.link(&squad_path(&squad.id)),
        squad,
        roster,
    };
    Ok((status, page).into_response())
}

#[derive(Deserialize)]
struct AddForm {
    character: String,
    #[serde(default)]
    tags: String,
    #[serde(default)]
    filter: Option<String>,
}

async fn add_character(
    State(state): State<AppState>,
    squad: db::Squad,
    htmx: Htmx,
    origin: Origin,
    Form(form): Form<AddForm>,
) -> Result<Response> {
    let parsed = form
        .character
        .parse::<CharacterName>()
        .map_err(|e| e.to_string())
        .and_then(|c| Ok((c, parse_tags(&form.tags).map_err(|e| e.to_string())?)));
    let error = match parsed {
        Ok((character, tags)) => {
            if db::add_character(&state.db, &squad.id, squad.region, &character, &tags).await? {
                state.wake.notify_one();
                None
            } else {
                Some(format!("{character} is already in this squad"))
            }
        }
        Err(error) => Some(error),
    };
    let outcome = match error {
        None => Outcome::default(),
        Some(error) => Outcome {
            error: Some(error),
            character_input: form.character,
            tags_input: form.tags,
            ..Outcome::default()
        },
    };
    respond(&state, htmx, &origin, squad, form.filter, outcome).await
}

#[derive(Deserialize)]
struct TagsForm {
    #[serde(default)]
    tags: String,
    #[serde(default)]
    filter: Option<String>,
}

/// The rest of a path below `/s/{squad}`; the squad itself is the `db::Squad` extractor's.
#[derive(Deserialize)]
struct CharacterPath {
    id: i64,
}

#[derive(Deserialize)]
struct TagPath {
    tag: String,
}

#[derive(Deserialize)]
struct CharacterTagPath {
    id: i64,
    tag: String,
}

async fn add_tags(
    State(state): State<AppState>,
    squad: db::Squad,
    Path(path): Path<CharacterPath>,
    htmx: Htmx,
    origin: Origin,
    Form(form): Form<TagsForm>,
) -> Result<Response> {
    let outcome = match parse_tags(&form.tags) {
        Ok(tags) => {
            db::add_tags(&state.db, &squad.id, path.id, &tags).await?;
            Outcome::default()
        }
        Err(e) => Outcome::error(e.to_string()),
    };
    respond(&state, htmx, &origin, squad, form.filter, outcome).await
}

#[derive(Deserialize)]
struct FilterForm {
    #[serde(default)]
    filter: Option<String>,
}

async fn remove_tag(
    State(state): State<AppState>,
    squad: db::Squad,
    Path(path): Path<CharacterTagPath>,
    htmx: Htmx,
    origin: Origin,
    Form(form): Form<FilterForm>,
) -> Result<Response> {
    db::remove_tag(&state.db, &squad.id, path.id, &path.tag).await?;
    respond(
        &state,
        htmx,
        &origin,
        squad,
        form.filter,
        Outcome::default(),
    )
    .await
}

#[derive(Deserialize)]
struct ColourForm {
    #[serde(default)]
    colour: String,
    /// Set by the reset button: back to the default colour.
    #[serde(default)]
    reset: Option<String>,
    #[serde(default)]
    filter: Option<String>,
}

async fn set_colour(
    State(state): State<AppState>,
    squad: db::Squad,
    Path(path): Path<TagPath>,
    htmx: Htmx,
    origin: Origin,
    Form(form): Form<ColourForm>,
) -> Result<Response> {
    let colour = if form.reset.is_some() {
        Ok(None)
    } else {
        Colour::parse(&form.colour)
            .map(Some)
            .map_err(|e| e.to_string())
    };
    let outcome = match (Tag::new(&path.tag).map_err(|e| e.to_string()), colour) {
        (Ok(None), _) => return Err(AppError::NotFound),
        (Ok(Some(tag)), Ok(colour)) => {
            db::set_tag_colour(&state.db, &squad.id, &tag, colour.as_ref()).await?;
            Outcome::default()
        }
        (Err(e), _) | (_, Err(e)) => Outcome::error(e),
    };
    respond(
        &state,
        htmx,
        &origin,
        squad,
        form.filter,
        Outcome {
            colours_open: true,
            ..outcome
        },
    )
    .await
}

async fn remove_character(
    State(state): State<AppState>,
    squad: db::Squad,
    Path(path): Path<CharacterPath>,
    htmx: Htmx,
    origin: Origin,
    Form(form): Form<FilterForm>,
) -> Result<Response> {
    db::remove_character(&state.db, &squad.id, path.id).await?;
    respond(
        &state,
        htmx,
        &origin,
        squad,
        form.filter,
        Outcome::default(),
    )
    .await
}

async fn refresh(
    State(state): State<AppState>,
    squad: db::Squad,
    Path(path): Path<CharacterPath>,
    htmx: Htmx,
    origin: Origin,
    Form(form): Form<FilterForm>,
) -> Result<Response> {
    db::request_refresh(&state.db, &squad.id, path.id).await?;
    state.wake.notify_one();
    respond(
        &state,
        htmx,
        &origin,
        squad,
        form.filter,
        Outcome::default(),
    )
    .await
}

// ---- auction prices ----

#[derive(Template, WebTemplate)]
#[template(path = "auctions.html")]
struct AuctionsPage {
    logout: bool,
    enabled: bool,
    houses: Vec<HouseView>,
    regions: Vec<RegionPrices>,
    /// The price lines' width and height.
    spark: (f64, f64),
}

/// A followed house and how collecting its prices is going.
struct HouseView {
    region: Region,
    id: u32,
    label: String,
    checked: Option<String>,
    error: Option<String>,
    snapshots: i64,
    since: Option<String>,
    last_items: i64,
}

/// Leather prices in one region's houses: a row per leather, a cell per house.
struct RegionPrices {
    region: Region,
    houses: Vec<auctions::House>,
    rows: Vec<(&'static str, Vec<Option<PriceCell>>)>,
}

struct PriceCell {
    /// The latest snapshot's median.
    now: Money,
    low: Money,
    high: Money,
    /// For the tooltip.
    summary: String,
    line: Option<Sparkline>,
}

/// How far back the leather prices go.
const PRICE_DAYS: i64 = 7;
/// The price lines' size in pixels, and how far their ends stay from the edges.
const SPARK_WIDTH: f64 = 96.0;
const SPARK_HEIGHT: f64 = 24.0;
const SPARK_PAD: f64 = 3.0;

async fn auctions_page(State(state): State<AppState>) -> Result<AuctionsPage> {
    let now = db::now();
    let since = now - PRICE_DAYS * auctions::DAY;
    let leather: Vec<u32> = auctions::LEATHER.iter().map(|&(id, _)| id).collect();
    let mut houses = Vec::new();
    let mut regions: Vec<RegionPrices> = Vec::new();
    for house in auctions::followed() {
        let status = db::house_status(&state.db, house.region, house.id).await?;
        houses.push(HouseView {
            region: house.region,
            id: house.id,
            label: house.label(),
            checked: status.checked_at.map(|at| ago(now - at)),
            error: status.error,
            snapshots: status.snapshots,
            since: status.first_at.map(|at| ago(now - at)),
            last_items: status.last_items,
        });

        let ranges = db::price_ranges(&state.db, house.region, house.id, &leather, since).await?;
        let series = db::price_series(&state.db, house.region, house.id, &leather, since).await?;
        if regions.last().is_none_or(|r| r.region != house.region) {
            regions.push(RegionPrices {
                region: house.region,
                houses: Vec::new(),
                rows: auctions::LEATHER
                    .iter()
                    .map(|&(_, name)| (name, Vec::new()))
                    .collect(),
            });
        }
        let Some(region) = regions.last_mut() else {
            continue;
        };
        region.houses.push(house);
        for (row, &(item, _)) in region.rows.iter_mut().zip(&auctions::LEATHER) {
            let points: Vec<(i64, u64)> = series
                .iter()
                .filter(|p| p.item == item)
                .map(|p| (p.at, p.median))
                .collect();
            row.1.push(
                ranges
                    .iter()
                    .find(|r| r.item == item)
                    .zip(points.last())
                    .map(|(r, &(_, latest))| PriceCell {
                        now: Money::from_copper(latest),
                        low: Money::brief(r.min),
                        high: Money::brief(r.max),
                        summary: format!(
                            "{PRICE_DAYS} days: average {}, lowest {}, highest {}, {} snapshots",
                            auctions::money(r.avg),
                            auctions::money(r.min),
                            auctions::money(r.max),
                            r.snapshots
                        ),
                        line: sparkline(&points, since, now, SPARK_WIDTH, SPARK_HEIGHT, SPARK_PAD),
                    }),
            );
        }
    }
    Ok(AuctionsPage {
        logout: state.auth.is_some(),
        enabled: state.auctions_enabled,
        houses,
        regions,
        spark: (SPARK_WIDTH, SPARK_HEIGHT),
    })
}

async fn healthz(State(state): State<AppState>) -> Response {
    match sqlx::query("SELECT 1").execute(&state.db).await {
        Ok(_) => (StatusCode::OK, "ok").into_response(),
        Err(e) => {
            tracing::error!("health check failed: {e}");
            (StatusCode::SERVICE_UNAVAILABLE, "database unavailable").into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ago_reads_naturally() {
        assert_eq!(ago(5), "just now");
        assert_eq!(ago(60), "1 minute ago");
        assert_eq!(ago(7200), "2 hours ago");
        assert_eq!(ago(86_400 * 3), "3 days ago");
    }

    #[test]
    fn urlencode_escapes_reserved() {
        assert_eq!(urlencode("main tank&co"), "main%20tank%26co");
        assert_eq!(urlencode("ä"), "%C3%A4");
    }
}
