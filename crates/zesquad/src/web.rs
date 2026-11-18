//! Pages and form handlers. Every form works as a plain POST with a redirect; with htmx the
//! squad page swaps just the roster instead of reloading.

use std::sync::Arc;

use askama::Template;
use askama_web::WebTemplate;
use axum::extract::{Form, Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use sqlx::SqlitePool;
use tokio::sync::Notify;
use zesquad_core::armory::{Item, Pet, Profession, Profile};
use zesquad_core::refresh::Status;
use zesquad_core::{CharacterName, SquadName, parse_tags};

use crate::db;
use crate::version;

#[derive(Clone)]
pub struct AppState {
    pub db: SqlitePool,
    pub wake: Arc<Notify>,
    pub region: String,
    pub armory_enabled: bool,
    /// Set when a password protects the app.
    pub auth: Option<Arc<crate::auth::Auth>>,
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/squads", post(create_squad))
        .route("/s/{squad}", get(squad_page))
        .route("/s/{squad}/roster", get(roster_partial))
        .route("/s/{squad}/name", post(rename))
        .route("/s/{squad}/characters", post(add_character))
        .route("/s/{squad}/characters/{id}/tags", post(add_tags))
        .route(
            "/s/{squad}/characters/{id}/tags/{tag}/delete",
            post(remove_tag),
        )
        .route("/s/{squad}/characters/{id}/delete", post(remove_character))
        .route("/s/{squad}/characters/{id}/refresh", post(refresh))
        .route("/healthz", get(healthz))
        .route(
            "/version",
            get(|| async { Json(serde_json::json!({ "version": version::short() })) }),
        )
        .route("/static/{version}/{*path}", get(crate::assets::serve))
        .route("/login", get(login_page).post(login))
        .route("/logout", post(logout))
        .fallback(|| async { AppError::NotFound })
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            crate::auth::require,
        ))
        .with_state(state)
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
    static_base: String,
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
            static_base: crate::assets::base(),
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
    pub tags: Vec<(String, String)>,
    pub status: &'static str,
    pub profile: Profile,
    /// The Armory answered at some point; still shown while a refresh is pending or failed.
    pub has_profile: bool,
    pub class_key: String,
    pub last_login: Option<String>,
    pub checked: Option<String>,
    pub error: Option<String>,
}

impl MemberView {
    fn new(m: db::Member, now: i64) -> Self {
        let profile = Profile::from_documents(&m.documents);
        Self {
            has_profile: m.documents.summary.is_some(),
            id: m.id,
            class_key: profile.class_key(),
            last_login: profile.last_login_ms.map(|ms| ago(now - ms / 1000)),
            checked: m.fetched_at.map(|t| ago(now - t)),
            name: profile.name.clone().unwrap_or(m.name),
            surname: profile.surname.clone().unwrap_or(m.surname),
            profile,
            tags: m.tags,
            status: m.status.as_str(),
            error: m.error,
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

    #[must_use]
    pub fn gear(&self) -> &[Item] {
        &self.profile.gear
    }

    #[must_use]
    pub fn professions(&self) -> &[Profession] {
        &self.profile.professions
    }

    #[must_use]
    pub fn pets(&self) -> &[Pet] {
        &self.profile.pets
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
    pub tags: Vec<(String, String)>,
    /// Key of the tag being filtered on.
    pub filter: Option<String>,
    pub armory_enabled: bool,
    pub error: Option<String>,
    pub character_input: String,
    pub tags_input: String,
}

impl Roster {
    /// The squad's URL, which every form posts below.
    #[must_use]
    pub fn base(&self) -> String {
        format!("/s/{}", self.squad_id)
    }

    /// While anything is waiting for its first fetch, the page polls for the result.
    #[must_use]
    pub fn polling(&self) -> bool {
        self.armory_enabled
            && self
                .members
                .iter()
                .any(|m| m.status == Status::Pending.as_str())
    }

    #[must_use]
    pub fn filter_query(&self) -> String {
        self.filter
            .as_ref()
            .map_or_else(String::new, |t| format!("?tag={}", urlencode(t)))
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

/// Percent-encodes everything but unreserved characters, for tags in URLs.
#[must_use]
pub fn urlencode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

#[derive(Template, WebTemplate)]
#[template(path = "index.html")]
struct IndexPage {
    static_base: String,
    /// Show the log-out button (a password is set).
    logout: bool,
    error: Option<String>,
    name_input: String,
}

#[derive(Template, WebTemplate)]
#[template(path = "squad.html")]
struct SquadPage {
    static_base: String,
    /// Show the log-out button (a password is set).
    logout: bool,
    squad: db::Squad,
    roster: Roster,
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
    static_base: String,
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
        static_base: crate::assets::base(),
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
        static_base: crate::assets::base(),
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

async fn index(State(state): State<AppState>) -> IndexPage {
    IndexPage {
        static_base: crate::assets::base(),
        logout: state.auth.is_some(),
        error: None,
        name_input: String::new(),
    }
}

#[derive(Deserialize)]
struct NameForm {
    name: String,
}

async fn create_squad(
    State(state): State<AppState>,
    Form(form): Form<NameForm>,
) -> Result<Response> {
    match SquadName::new(&form.name) {
        Ok(name) => {
            let id = db::create_squad(&state.db, &name).await?;
            Ok(Redirect::to(&format!("/s/{id}")).into_response())
        }
        Err(e) => Ok((
            StatusCode::UNPROCESSABLE_ENTITY,
            IndexPage {
                static_base: crate::assets::base(),
                logout: state.auth.is_some(),
                error: Some(e.to_string()),
                name_input: form.name,
            },
        )
            .into_response()),
    }
}

#[derive(Deserialize)]
struct FilterQuery {
    tag: Option<String>,
}

async fn load_squad(state: &AppState, id: &str) -> Result<db::Squad> {
    db::squad(&state.db, id).await?.ok_or(AppError::NotFound)
}

async fn build_roster(state: &AppState, squad_id: &str, filter: Option<String>) -> Result<Roster> {
    let filter = filter.filter(|t| !t.is_empty()).map(|t| t.to_lowercase());
    let now = db::now();
    let members = db::members(&state.db, squad_id, filter.as_deref())
        .await?
        .into_iter()
        .map(|m| MemberView::new(m, now))
        .collect();
    Ok(Roster {
        squad_id: squad_id.to_owned(),
        members,
        tags: db::squad_tags(&state.db, squad_id).await?,
        filter,
        armory_enabled: state.armory_enabled,
        error: None,
        character_input: String::new(),
        tags_input: String::new(),
    })
}

async fn squad_page(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<FilterQuery>,
) -> Result<SquadPage> {
    let squad = load_squad(&state, &id).await?;
    let roster = build_roster(&state, &squad.id, q.tag).await?;
    Ok(SquadPage {
        static_base: crate::assets::base(),
        logout: state.auth.is_some(),
        squad,
        roster,
    })
}

async fn roster_partial(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<FilterQuery>,
) -> Result<RosterPartial> {
    let squad = load_squad(&state, &id).await?;
    Ok(RosterPartial {
        roster: build_roster(&state, &squad.id, q.tag).await?,
    })
}

async fn rename(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Form(form): Form<NameForm>,
) -> Result<Redirect> {
    let squad = load_squad(&state, &id).await?;
    // An invalid name just keeps the old one; the field is required in the form anyway.
    if let Ok(name) = SquadName::new(&form.name) {
        db::rename_squad(&state.db, &squad.id, &name).await?;
    }
    Ok(Redirect::to(&format!("/s/{}", squad.id)))
}

/// After a change: htmx gets the new roster, a plain form post a redirect back to the page.
/// An error (bad input) is shown in the roster, keeping what was typed.
struct Outcome {
    error: Option<String>,
    character_input: String,
    tags_input: String,
}

impl Outcome {
    fn ok() -> Self {
        Self {
            error: None,
            character_input: String::new(),
            tags_input: String::new(),
        }
    }
}

async fn respond(
    state: &AppState,
    headers: &HeaderMap,
    squad: db::Squad,
    filter: Option<String>,
    outcome: Outcome,
) -> Result<Response> {
    let htmx = headers.contains_key("hx-request");
    if !htmx && outcome.error.is_none() {
        let query = filter
            .filter(|t| !t.is_empty())
            .map_or_else(String::new, |t| format!("?tag={}", urlencode(&t)));
        return Ok(Redirect::to(&format!("/s/{}{query}", squad.id)).into_response());
    }
    let mut roster = build_roster(state, &squad.id, filter).await?;
    let failed = outcome.error.is_some();
    roster.error = outcome.error;
    roster.character_input = outcome.character_input;
    roster.tags_input = outcome.tags_input;
    if htmx {
        return Ok(RosterPartial { roster }.into_response());
    }
    let status = if failed {
        StatusCode::UNPROCESSABLE_ENTITY
    } else {
        StatusCode::OK
    };
    let page = SquadPage {
        static_base: crate::assets::base(),
        logout: state.auth.is_some(),
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
    Path(id): Path<String>,
    headers: HeaderMap,
    Form(form): Form<AddForm>,
) -> Result<Response> {
    let squad = load_squad(&state, &id).await?;
    let parsed = form
        .character
        .parse::<CharacterName>()
        .map_err(|e| e.to_string())
        .and_then(|c| Ok((c, parse_tags(&form.tags).map_err(|e| e.to_string())?)));
    let outcome = match parsed {
        Ok((character, tags)) => {
            if db::add_character(&state.db, &squad.id, &state.region, &character, &tags).await? {
                state.wake.notify_one();
                Outcome::ok()
            } else {
                Outcome {
                    error: Some(format!("{character} is already in this squad")),
                    character_input: form.character,
                    tags_input: form.tags,
                }
            }
        }
        Err(error) => Outcome {
            error: Some(error),
            character_input: form.character,
            tags_input: form.tags,
        },
    };
    respond(&state, &headers, squad, form.filter, outcome).await
}

#[derive(Deserialize)]
struct TagsForm {
    #[serde(default)]
    tags: String,
    #[serde(default)]
    filter: Option<String>,
}

async fn add_tags(
    State(state): State<AppState>,
    Path((id, character)): Path<(String, i64)>,
    headers: HeaderMap,
    Form(form): Form<TagsForm>,
) -> Result<Response> {
    let squad = load_squad(&state, &id).await?;
    let outcome = match parse_tags(&form.tags) {
        Ok(tags) => {
            db::add_tags(&state.db, &squad.id, character, &tags).await?;
            Outcome::ok()
        }
        Err(e) => Outcome {
            error: Some(e.to_string()),
            ..Outcome::ok()
        },
    };
    respond(&state, &headers, squad, form.filter, outcome).await
}

#[derive(Deserialize)]
struct FilterForm {
    #[serde(default)]
    filter: Option<String>,
}

async fn remove_tag(
    State(state): State<AppState>,
    Path((id, character, tag)): Path<(String, i64, String)>,
    headers: HeaderMap,
    Form(form): Form<FilterForm>,
) -> Result<Response> {
    let squad = load_squad(&state, &id).await?;
    db::remove_tag(&state.db, &squad.id, character, &tag).await?;
    respond(&state, &headers, squad, form.filter, Outcome::ok()).await
}

async fn remove_character(
    State(state): State<AppState>,
    Path((id, character)): Path<(String, i64)>,
    headers: HeaderMap,
    Form(form): Form<FilterForm>,
) -> Result<Response> {
    let squad = load_squad(&state, &id).await?;
    db::remove_character(&state.db, &squad.id, character).await?;
    respond(&state, &headers, squad, form.filter, Outcome::ok()).await
}

async fn refresh(
    State(state): State<AppState>,
    Path((id, character)): Path<(String, i64)>,
    headers: HeaderMap,
    Form(form): Form<FilterForm>,
) -> Result<Response> {
    let squad = load_squad(&state, &id).await?;
    db::request_refresh(&state.db, &squad.id, character).await?;
    state.wake.notify_one();
    respond(&state, &headers, squad, form.filter, Outcome::ok()).await
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

/// Static files under a versioned path never change, so browsers may keep them for a year.
#[must_use]
pub fn cache_forever() -> (header::HeaderName, HeaderValue) {
    (
        header::CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=31536000, immutable"),
    )
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
