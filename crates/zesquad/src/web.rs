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
use zesquad_core::access::{self, Action, Role, Visibility};
use zesquad_core::armory::Profile;
use zesquad_core::auctions::{self, Money};
use zesquad_core::discord::Webhook;
use zesquad_core::recent::Recent;
use zesquad_core::refresh::Status;
use zesquad_core::roster::{self as order, Sort};
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
    /// Set when people can log in with Battle.net.
    pub login: Option<Arc<crate::login::Login>>,
    /// Posts squads' news to their Discord channels.
    pub discord: Arc<crate::discord::Discord>,
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
        .route("/auth/login", get(start_login))
        .route("/auth/callback", get(finish_login))
        .route("/account", get(account_page))
        .route("/account/sessions/delete", post(log_out_everywhere))
        .route("/account/delete", post(delete_account))
        .route("/privacy", get(privacy_page))
        .route("/squads", post(create_squad))
        .route("/recent/{squad}/forget", post(forget_squad))
        .route("/s/{squad}", get(squad_page))
        .route("/s/{squad}/roster", get(roster_partial))
        .route("/s/{squad}/name", post(rename))
        .route("/s/{squad}/discord", post(set_webhook))
        .route("/s/{squad}/discord/delete", post(remove_webhook))
        .route("/s/{squad}/invite/reset", post(reset_invite))
        .route("/s/{squad}/visibility", post(set_visibility))
        .route("/join/{token}", get(join_page).post(join))
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
            crate::login::load,
        ))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            crate::auth::require,
        ));
    open.merge(protected).with_state(state)
}

/// A squad from the `{squad}` part of the path, and what the person asking may do with it.
pub struct SquadAccess {
    pub squad: db::Squad,
    /// Their role in it; `None` when logged out or not in it.
    pub role: Option<Role>,
    /// The visibility the rule goes by: the squad's, or `Open` with accounts off.
    visibility: Visibility,
}

impl SquadAccess {
    #[must_use]
    pub fn allows(&self, action: Action) -> bool {
        access::allows(self.visibility, self.role, action)
    }

    /// Loads the squad and applies the rule for `action`: a squad you can't see is a 404, the
    /// same as one that doesn't exist; one you can see but not change this way is a 403.
    async fn load(parts: &mut Parts, state: &AppState, action: Action) -> Result<Self> {
        let params = RawPathParams::from_request_parts(parts, state)
            .await
            .map_err(|_| AppError::NotFound)?;
        let id = params
            .iter()
            .find_map(|(name, value)| (name == "squad").then_some(value))
            .ok_or(AppError::NotFound)?;
        let mut squad = db::squad(&state.db, id).await?.ok_or(AppError::NotFound)?;
        let user = parts
            .extensions
            .get::<crate::login::CurrentUser>()
            .and_then(|u| u.0.as_ref());
        // A squad from before accounts goes to the first person logged in to open it.
        if let (Some(_), Some(user), Visibility::Open) = (&state.login, user, squad.visibility)
            && db::claim(&state.db, &squad.id, user.id).await?
        {
            tracing::info!(user = user.id, squad = squad.id, "claimed a squad");
            squad = db::squad(&state.db, id).await?.ok_or(AppError::NotFound)?;
        }
        let (visibility, role) = match (&state.login, user) {
            (None, _) => (Visibility::Open, None),
            (Some(_), None) => (squad.visibility, None),
            (Some(_), Some(user)) => (
                squad.visibility,
                db::role(&state.db, &squad.id, user.id).await?,
            ),
        };
        let access = Self {
            squad,
            role,
            visibility,
        };
        if !access.allows(Action::View) {
            return Err(AppError::NotFound);
        }
        if !access.allows(action) {
            return Err(AppError::Forbidden {
                htmx: Htmx::of(&parts.headers),
            });
        }
        Ok(access)
    }
}

/// Extractors for a squad route, one per thing it does; each loads the squad and refuses
/// anyone the rule (`zesquad_core::access`) doesn't allow. Every squad route takes one, so a
/// new route can't forget the check.
macro_rules! squad_extractor {
    ($(#[$doc:meta])* $name:ident, $action:expr) => {
        $(#[$doc])*
        pub struct $name(pub SquadAccess);

        impl FromRequestParts<AppState> for $name {
            type Rejection = AppError;

            async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self> {
                SquadAccess::load(parts, state, $action).await.map(Self)
            }
        }
    };
}

squad_extractor!(
    /// A squad you can see.
    CanView,
    Action::View
);
squad_extractor!(
    /// A squad whose roster you can change.
    CanEdit,
    Action::Edit
);
squad_extractor!(
    /// A squad you can rename and point at a Discord channel.
    CanConfigure,
    Action::Configure
);
squad_extractor!(
    /// A squad you own.
    CanOwn,
    Action::Own
);

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

/// What every page's header and links need from the request: who's logged in, how to log in
/// or out, and the `scheme://host` the request was sent to, as the browser sees it (through a
/// proxy too; `None` when the host doesn't look like one).
#[derive(Clone, Default)]
pub struct Nav {
    /// A shared password is set (log out of it).
    pub password: bool,
    /// Logging in with Battle.net is on.
    pub login: bool,
    pub user: Option<db::User>,
    /// This page, to come back to after logging in.
    pub here: String,
    origin: Option<String>,
}

impl Nav {
    fn of(state: &AppState, parts: &Parts) -> Self {
        let headers = &parts.headers;
        let host = ["x-forwarded-host", header::HOST.as_str()]
            .iter()
            .find_map(|name| headers.get(*name)?.to_str().ok());
        let https = !crate::auth::secure(headers).is_empty();
        let user = parts
            .extensions
            .get::<crate::login::CurrentUser>()
            .and_then(|u| u.0.clone());
        let here = if parts.method == axum::http::Method::GET {
            parts.uri.path_and_query().map_or("/", |p| p.as_str())
        } else {
            parts.uri.path()
        };
        Self {
            password: state.auth.is_some(),
            login: state.login.is_some(),
            user,
            here: zesquad_core::redirect::safe_next(Some(here)).to_owned(),
            origin: host.and_then(|host| zesquad_core::link::origin(https, host)),
        }
    }

    /// The full link to `path`; just the path without an origin.
    fn link(&self, path: &str) -> String {
        format!("{}{path}", self.origin.as_deref().unwrap_or_default())
    }

    /// Show a log-out button: logged in, or past the shared password.
    #[must_use]
    pub fn can_log_out(&self) -> bool {
        self.user.is_some() || self.password
    }

    /// Log in, then come back here.
    #[must_use]
    pub fn login_link(&self) -> String {
        format!("/auth/login?next={}", urlencode(&self.here))
    }
}

impl FromRequestParts<AppState> for Nav {
    type Rejection = std::convert::Infallible;

    fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> impl Future<Output = std::result::Result<Self, Self::Rejection>> + Send {
        std::future::ready(Ok(Self::of(state, parts)))
    }
}

/// Where an invite link points.
#[must_use]
pub fn join_path(token: &str) -> String {
    format!("/join/{token}")
}

/// A squad's page; every form on it posts below this.
#[must_use]
pub fn squad_path(id: &str) -> String {
    format!("/s/{id}")
}

// ---- errors ----

pub enum AppError {
    NotFound,
    /// Not allowed to change this; `htmx` asked from a page that showed forms it shouldn't
    /// have (a session that ended, a membership that did), so it gets the page reloaded.
    Forbidden {
        htmx: bool,
    },
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
    nav: Nav,
    title: &'static str,
    message: &'static str,
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, title, message) = match self {
            Self::Forbidden { htmx: true } => {
                // htmx doesn't swap a 403, but it does follow HX-Refresh on a 200: the reload
                // shows the page as this person may see it.
                return (StatusCode::OK, [("hx-refresh", "true")]).into_response();
            }
            Self::Forbidden { htmx: false } => (
                StatusCode::FORBIDDEN,
                "Only members can change this squad",
                "Ask a member of the squad for its invite link, then log in and join.",
            ),
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
            nav: Nav::default(),
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

    /// What the roster sorts this character by; `added` is its place in the squad.
    fn sort_entry(&self, added: usize) -> order::Entry {
        let number = |n: Option<u64>| n.and_then(|n| i64::try_from(n).ok());
        order::Entry {
            added,
            name: format!("{} {}", self.name, self.surname).to_lowercase(),
            level: number(self.profile.level),
            item_level: number(self.profile.item_level),
            last_played: self.profile.last_login_ms,
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
    pub sort: Sort,
    /// "6 characters · 4 at level 60 · …", for the members shown.
    pub summary: String,
    pub armory_enabled: bool,
    /// Forms and links to change things are shown; the front page's example has none.
    pub editable: bool,
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

    /// `?tag=…&sort=…` for the current filter and sort, empty for neither.
    #[must_use]
    pub fn query(&self) -> String {
        view_query(self.filter.as_deref(), self.sort)
    }

    /// The squad page filtered on `key`, sorted as now.
    #[must_use]
    pub fn tag_link(&self, key: &str) -> String {
        format!("{}{}", self.base(), view_query(Some(key), self.sort))
    }

    /// The squad page without a filter, sorted as now.
    #[must_use]
    pub fn all_link(&self) -> String {
        format!("{}{}", self.base(), view_query(None, self.sort))
    }

    /// The squad page sorted by `sort`, filtered as now.
    #[must_use]
    pub fn sort_link(&self, sort: Sort) -> String {
        format!(
            "{}{}",
            self.base(),
            view_query(self.filter.as_deref(), sort)
        )
    }

    #[must_use]
    pub fn is_sort(&self, sort: Sort) -> bool {
        self.sort == sort
    }

    #[must_use]
    pub fn sort_value(&self) -> &'static str {
        self.sort.as_str()
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

/// `?tag=<key>&sort=<sort>` for a tag filter and a sort; each left out when not set (the
/// default sort), so plain links stay plain.
fn view_query(filter: Option<&str>, sort: Sort) -> String {
    let parts: Vec<String> = [
        filter.map(|t| format!("tag={}", urlencode(t))),
        (sort != Sort::default()).then(|| format!("sort={}", sort.as_str())),
    ]
    .into_iter()
    .flatten()
    .collect();
    if parts.is_empty() {
        String::new()
    } else {
        format!("?{}", parts.join("&"))
    }
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
    nav: Nav,
    /// Squads this browser opened lately, newest first.
    recent: Vec<db::SquadSummary>,
    /// Example characters showing what a squad looks like.
    example: Roster,
    error: Option<String>,
    name_input: String,
    /// The region picked in the form.
    region: Region,
}

#[derive(Template, WebTemplate)]
#[template(path = "squad.html")]
struct SquadPage {
    nav: Nav,
    access: SquadAccess,
    /// The page's full link, to copy; just the path when the request's host looks wrong.
    link: String,
    /// The invite link, for members to pass on.
    invite: Option<String>,
    roster: Roster,
    /// Why the Discord webhook someone pasted wasn't kept.
    discord_error: Option<String>,
}

impl SquadPage {
    fn new(nav: Nav, access: SquadAccess, roster: Roster) -> Self {
        let invite = access
            .squad
            .invite_token
            .as_deref()
            .filter(|_| access.allows(Action::SeeMembers))
            .map(|token| nav.link(&join_path(token)));
        Self {
            link: nav.link(&squad_path(&access.squad.id)),
            invite,
            nav,
            access,
            roster,
            discord_error: None,
        }
    }

    /// Link previews may name the squad: not when it's private, in case one is ever made with
    /// a member's cookies.
    fn shows_name_in_previews(&self) -> bool {
        self.access.squad.visibility != Visibility::Private
    }

    /// The squad's own link is worth sharing: anyone but members would get a 404 for a
    /// private one.
    fn shows_view_link(&self) -> bool {
        self.access.squad.visibility != Visibility::Private
    }

    /// The end of the squad's Discord webhook id, to recognise it by; never the token.
    fn webhook_hint(&self) -> Option<String> {
        self.access.squad.webhook().map(|hook| hook.hint())
    }

    /// One line about the squad for link previews: "6 characters in Europe on zesquad, …".
    fn summary(&self) -> String {
        let n = self.roster.total;
        format!(
            "{n} character{} in {} on zesquad, friend squads for WoW: Forever.",
            if n == 1 { "" } else { "s" },
            self.access.squad.region.name()
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
    nav: Nav,
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
        nav: Nav::default(),
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
        nav: Nav::default(),
        next,
        wrong: true,
    };
    (StatusCode::UNAUTHORIZED, page).into_response()
}

/// Logs out of everything: the Battle.net session and the shared password. Back to the
/// password page when there is one, else the front page.
async fn logout(State(state): State<AppState>, headers: HeaderMap) -> Result<Response> {
    crate::login::end_session(&state, &headers).await?;
    let to = if state.auth.is_some() { "/login" } else { "/" };
    Ok((
        AppendHeaders([
            (header::SET_COOKIE, crate::auth::logout_cookie()),
            (header::SET_COOKIE, crate::login::logout_cookie()),
        ]),
        Redirect::to(to),
    )
        .into_response())
}

// ---- your account ----

#[derive(Template, WebTemplate)]
#[template(path = "account.html")]
struct AccountPage {
    nav: Nav,
    /// When you first logged in, as "3 days ago".
    joined: String,
}

#[derive(Template, WebTemplate)]
#[template(path = "privacy.html")]
struct PrivacyPage {
    nav: Nav,
}

async fn privacy_page(nav: Nav) -> PrivacyPage {
    PrivacyPage { nav }
}

/// Your account, or the login when you aren't logged in.
async fn account_page(State(state): State<AppState>, nav: Nav) -> Result<Response> {
    let Some(user) = &nav.user else {
        return Ok(login_first(&state, &nav));
    };
    let joined = ago(db::now() - db::user_created(&state.db, user.id).await?);
    Ok(AccountPage { nav, joined }.into_response())
}

/// Sends someone who isn't logged in to log in, and back here after; a 404 when logging in is
/// off, since then nobody has an account.
fn login_first(state: &AppState, nav: &Nav) -> Response {
    if state.login.is_none() {
        return AppError::NotFound.into_response();
    }
    Redirect::to(&nav.login_link()).into_response()
}

async fn log_out_everywhere(State(state): State<AppState>, nav: Nav) -> Result<Response> {
    let Some(user) = &nav.user else {
        return Ok(login_first(&state, &nav));
    };
    db::delete_sessions(&state.db, user.id).await?;
    Ok((
        [(header::SET_COOKIE, crate::login::logout_cookie())],
        Redirect::to("/"),
    )
        .into_response())
}

async fn delete_account(State(state): State<AppState>, nav: Nav) -> Result<Response> {
    let Some(user) = &nav.user else {
        return Ok(login_first(&state, &nav));
    };
    db::delete_user(&state.db, user.id).await?;
    tracing::info!(user = user.id, "account deleted");
    Ok((
        [(header::SET_COOKIE, crate::login::logout_cookie())],
        Redirect::to("/"),
    )
        .into_response())
}

// ---- logging in with Battle.net ----

#[derive(Deserialize)]
struct StartLogin {
    next: Option<String>,
}

/// Sends you to Battle.net's login, remembering where to come back to.
async fn start_login(
    State(state): State<AppState>,
    Query(q): Query<StartLogin>,
) -> Result<Response> {
    let login = state.login.as_ref().ok_or(AppError::NotFound)?;
    let next = zesquad_core::redirect::safe_next(q.next.as_deref());
    let login_state = crate::login::random_token();
    let cookie = format!(
        "{}={}; Path=/auth; HttpOnly; SameSite=Lax; Max-Age=600{}",
        crate::login::LOGIN_COOKIE,
        zesquad_core::account::login_cookie(&login_state, next),
        login.secure()
    );
    let to = login.authorize_url(&login_state)?;
    Ok(([(header::SET_COOKIE, cookie)], Redirect::to(to.as_str())).into_response())
}

#[derive(Deserialize)]
struct Callback {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
}

/// Where Battle.net sends you back: checks the state, asks who you are, starts a session.
async fn finish_login(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<Callback>,
) -> Result<Response> {
    let login = state.login.as_ref().ok_or(AppError::NotFound)?;
    let clear = format!(
        "{}=; Path=/auth; HttpOnly; SameSite=Lax; Max-Age=0",
        crate::login::LOGIN_COOKIE
    );
    let remembered = crate::auth::cookie(&headers, crate::login::LOGIN_COOKIE)
        .and_then(zesquad_core::account::read_login_cookie);
    let refusal = |status, title, message| {
        let page = ErrorPage {
            nav: Nav::default(),
            title,
            message,
        };
        Ok(([(header::SET_COOKIE, clear.clone())], (status, page)).into_response())
    };
    let Some((expected, next)) = remembered else {
        return refusal(
            StatusCode::BAD_REQUEST,
            "Login expired",
            "That login took too long or didn't start here. Log in again from any page.",
        );
    };
    if q.error.is_some() {
        return refusal(
            StatusCode::FORBIDDEN,
            "Not logged in",
            "Battle.net didn't log you in, so nothing was shared with zesquad.",
        );
    }
    let state_matches = q
        .state
        .as_deref()
        .is_some_and(|s| crate::auth::constant_time_eq(s.as_bytes(), expected.as_bytes()));
    let (true, Some(code)) = (state_matches, q.code.as_deref()) else {
        return refusal(
            StatusCode::BAD_REQUEST,
            "Login expired",
            "That login took too long or didn't start here. Log in again from any page.",
        );
    };
    let identity = match login.identify(code).await {
        Ok(identity) => identity,
        Err(e) => {
            tracing::warn!("logging in: {e:#}");
            return refusal(
                StatusCode::BAD_GATEWAY,
                "Couldn't log in",
                "Battle.net didn't answer as expected. Try again in a moment.",
            );
        }
    };
    let user = db::upsert_user(&state.db, &identity).await?;
    let session = crate::login::start_session(&state, login, &user).await?;
    Ok((
        AppendHeaders([(header::SET_COOKIE, session), (header::SET_COOKIE, clear)]),
        Redirect::to(&next),
    )
        .into_response())
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

/// The line above a roster for `members` out of `total`.
fn summarise(members: &[MemberView], total: usize) -> String {
    let counted: Vec<order::Counted> = members
        .iter()
        .map(|m| order::Counted {
            level: m.profile.level,
            faction: m.profile.faction.clone(),
        })
        .collect();
    order::summary(&counted, total)
}

/// Made-up characters for the front page, shown with the same card as a real squad's.
fn example_roster() -> Roster {
    const HOUR_MS: i64 = 3_600_000;
    let now = db::now();
    let profession = |name: &str, skill| zesquad_core::armory::Profession {
        name: name.into(),
        skill,
        max: 300,
    };
    let member = |id,
                  (name, surname): (&str, &str),
                  (level, race, class, faction): (u64, &str, &str, &str),
                  guild: Option<&str>,
                  item_level,
                  talents: [(&str, u64); 2],
                  professions: Vec<zesquad_core::armory::Profession>,
                  (hours_ago, tags): (i64, &[&str])| {
        let profile = Profile {
            name: Some(name.into()),
            surname: Some(surname.into()),
            level: Some(level),
            race: Some(race.into()),
            class: Some(class.into()),
            faction: Some(faction.into()),
            guild: guild.map(Into::into),
            item_level: Some(item_level),
            last_login_ms: Some((now * 1000) - hours_ago * HOUR_MS),
            talents: talents.iter().map(|(t, p)| ((*t).into(), *p)).collect(),
            professions,
            ..Profile::default()
        };
        let m = db::Member {
            id,
            name: name.into(),
            surname: surname.into(),
            tags: tags
                .iter()
                .map(|t| ((*t).into(), t.to_lowercase()))
                .collect(),
            status: Status::Found,
            fetched_at: Some(now),
            armory: db::Armory {
                profile: Arc::new(profile),
                answered: true,
            },
        };
        MemberView::new(m, now, &Colours(std::collections::HashMap::new()))
    };
    let members = vec![
        member(
            1,
            ("Maelis", "Duskwhisper"),
            (60, "Night Elf", "Priest", "ALLIANCE"),
            Some("Lanterns of Ashwood"),
            58,
            [("Holy", 31), ("Discipline", 20)],
            vec![profession("Tailoring", 300), profession("Enchanting", 285)],
            (2, &["raid", "healer"]),
        ),
        member(
            2,
            ("Torvin", "Emberforge"),
            (60, "Dwarf", "Warrior", "ALLIANCE"),
            Some("Lanterns of Ashwood"),
            61,
            [("Protection", 31), ("Arms", 20)],
            vec![profession("Mining", 300), profession("Blacksmithing", 300)],
            (26, &["raid", "tank"]),
        ),
        member(
            3,
            ("Sylra", "Thornbrook"),
            (47, "Gnome", "Mage", "ALLIANCE"),
            None,
            39,
            [("Frost", 26), ("Arcane", 10)],
            vec![profession("Herbalism", 210)],
            (120, &["alt"]),
        ),
    ];
    Roster {
        squad_id: "example".into(),
        summary: summarise(&members, members.len()),
        total: members.len(),
        members,
        tags: Vec::new(),
        filter: None,
        sort: Sort::default(),
        armory_enabled: true,
        editable: false,
        form: Outcome::default(),
    }
}

async fn index(State(state): State<AppState>, headers: HeaderMap, nav: Nav) -> Result<Response> {
    let (recent, cookie) = recent_squads(&state, &headers).await?;
    let page = IndexPage {
        nav,
        recent,
        example: example_roster(),
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
    nav: Nav,
    Form(form): Form<NewSquadForm>,
) -> Result<Response> {
    let region = form
        .region
        .as_deref()
        .map_or(Ok(state.default_region), str::parse);
    let parsed = SquadName::new(&form.name)
        .map_err(|e| e.to_string())
        .and_then(|name| Ok((name, region.clone().map_err(|e| e.to_string())?)));
    // With accounts on, a squad belongs to whoever starts it, so that takes a login.
    let owner = match (&state.login, &nav.user) {
        (None, _) => None,
        (Some(_), Some(user)) => Some(user.id),
        (Some(_), None) => return Ok(Redirect::to("/auth/login?next=%2F").into_response()),
    };
    match parsed {
        Ok((name, region)) => {
            let id = db::create_squad(&state.db, &name, region, owner).await?;
            Ok(Redirect::to(&squad_path(&id)).into_response())
        }
        Err(error) => {
            let (recent, _) = recent_squads(&state, &headers).await?;
            let page = IndexPage {
                nav,
                recent,
                example: example_roster(),
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
    sort: Option<String>,
}

/// The roster with `filter`, a tag key from `tag_filter`, applied, in `sort`'s order.
async fn build_roster(
    state: &AppState,
    access: &SquadAccess,
    filter: Option<String>,
    sort: Sort,
    form: Outcome,
) -> Result<Roster> {
    let squad_id = access.squad.id.as_str();
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
    // Members come in the order they were added.
    let mut entries: Vec<(order::Entry, MemberView)> = members
        .into_iter()
        .enumerate()
        .map(|(added, m)| (m.sort_entry(added), m))
        .collect();
    entries.sort_by(|(a, _), (b, _)| sort.compare(a, b));
    let members: Vec<MemberView> = entries.into_iter().map(|(_, m)| m).collect();
    Ok(Roster {
        squad_id: squad_id.to_owned(),
        summary: summarise(&members, total),
        members,
        total,
        tags,
        filter,
        sort,
        armory_enabled: state.armory_enabled,
        editable: access.allows(Action::Edit),
        form,
    })
}

async fn squad_page(
    State(state): State<AppState>,
    headers: HeaderMap,
    nav: Nav,
    CanView(access): CanView,
    Query(q): Query<FilterQuery>,
) -> Result<Response> {
    let roster = build_roster(
        &state,
        &access,
        tag_filter(q.tag),
        Sort::parse(q.sort.as_deref()),
        Outcome::default(),
    )
    .await?;
    let mut recent = recent(&headers);
    recent.visit(&access.squad.id);
    let page = SquadPage::new(nav, access, roster);
    Ok(([recent_cookie(&headers, &recent)], page).into_response())
}

async fn roster_partial(
    State(state): State<AppState>,
    CanView(access): CanView,
    Query(q): Query<FilterQuery>,
) -> Result<RosterPartial> {
    Ok(RosterPartial {
        roster: build_roster(
            &state,
            &access,
            tag_filter(q.tag),
            Sort::parse(q.sort.as_deref()),
            Outcome::default(),
        )
        .await?,
    })
}

async fn rename(
    State(state): State<AppState>,
    CanConfigure(access): CanConfigure,
    Form(form): Form<NameForm>,
) -> Result<Redirect> {
    let squad = &access.squad;
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
    nav: &Nav,
    access: SquadAccess,
    view: &ViewForm,
    outcome: Outcome,
) -> Result<Response> {
    let filter = tag_filter(view.filter.clone());
    let sort = Sort::parse(view.sort.as_deref());
    let failed = outcome.error.is_some();
    if !htmx && !failed {
        let query = view_query(filter.as_deref(), sort);
        return Ok(
            Redirect::to(&format!("{}{query}", squad_path(&access.squad.id))).into_response(),
        );
    }
    let roster = build_roster(state, &access, filter, sort, outcome).await?;
    if htmx {
        return Ok(RosterPartial { roster }.into_response());
    }
    let status = if failed {
        StatusCode::UNPROCESSABLE_ENTITY
    } else {
        StatusCode::OK
    };
    Ok((status, SquadPage::new(nav.clone(), access, roster)).into_response())
}

// ---- invites ----

#[derive(Template, WebTemplate)]
#[template(path = "join.html")]
struct JoinPage {
    nav: Nav,
    squad: db::Squad,
    token: String,
}

/// The squad behind an invite link, when logging in is on and the token is current.
async fn invited(state: &AppState, token: &str) -> Result<db::Squad> {
    if state.login.is_none() {
        return Err(AppError::NotFound);
    }
    db::squad_by_invite(&state.db, token)
        .await?
        .ok_or(AppError::NotFound)
}

/// An invite link: log in first, then a page asking to join. Opening it never joins, so a chat
/// app previewing the link can't join anyone.
async fn join_page(
    State(state): State<AppState>,
    nav: Nav,
    Path(token): Path<String>,
) -> Result<Response> {
    let squad = invited(&state, &token).await?;
    let Some(user) = &nav.user else {
        return Ok(login_first(&state, &nav));
    };
    if db::role(&state.db, &squad.id, user.id).await?.is_some() {
        return Ok(Redirect::to(&squad_path(&squad.id)).into_response());
    }
    Ok(JoinPage { nav, squad, token }.into_response())
}

async fn join(
    State(state): State<AppState>,
    nav: Nav,
    Path(token): Path<String>,
) -> Result<Response> {
    let squad = invited(&state, &token).await?;
    let Some(user) = &nav.user else {
        return Ok(login_first(&state, &nav));
    };
    db::join(&state.db, &squad.id, user.id).await?;
    tracing::info!(user = user.id, squad = squad.id, "joined a squad");
    Ok(Redirect::to(&squad_path(&squad.id)).into_response())
}

#[derive(Deserialize)]
struct VisibilityForm {
    visibility: String,
}

/// Who can see the squad, one of [`Visibility::CHOICES`]; anything else (`open` included,
/// which is only for squads from before accounts) changes nothing.
async fn set_visibility(
    State(state): State<AppState>,
    CanOwn(access): CanOwn,
    Form(form): Form<VisibilityForm>,
) -> Result<Redirect> {
    if let Ok(visibility) = form.visibility.parse::<Visibility>()
        && Visibility::CHOICES.contains(&visibility)
    {
        db::set_visibility(&state.db, &access.squad.id, visibility).await?;
    }
    Ok(Redirect::to(&squad_path(&access.squad.id)))
}

async fn reset_invite(State(state): State<AppState>, CanOwn(access): CanOwn) -> Result<Redirect> {
    db::reset_invite(&state.db, &access.squad.id).await?;
    Ok(Redirect::to(&squad_path(&access.squad.id)))
}

// ---- Discord ----

#[derive(Deserialize)]
struct WebhookForm {
    #[serde(default)]
    webhook: String,
}

/// Sets the squad's Discord webhook, after posting a hello there so whoever pastes it sees it
/// works. A bad URL or one Discord refuses comes back as an error and isn't kept.
async fn set_webhook(
    State(state): State<AppState>,
    nav: Nav,
    CanConfigure(access): CanConfigure,
    Form(form): Form<WebhookForm>,
) -> Result<Response> {
    let squad = &access.squad;
    let error = match Webhook::parse(&form.webhook) {
        Err(e) => Some(e.to_string()),
        Ok(hook) => {
            let link = state.discord.squad_link(&squad.id);
            let hello = zesquad_core::discord::hello(&squad.name, link.as_deref());
            match state.discord.post(&hook, &hello).await {
                Ok(()) => {
                    db::set_webhook(&state.db, &squad.id, Some(&hook)).await?;
                    None
                }
                Err(e) => {
                    tracing::info!("discord webhook refused: {e:#}");
                    Some(
                        "Discord didn't take a message on that webhook: check it still exists, \
                         or copy its URL again"
                            .to_owned(),
                    )
                }
            }
        }
    };
    let Some(error) = error else {
        return Ok(Redirect::to(&squad_path(&squad.id)).into_response());
    };
    let roster = build_roster(&state, &access, None, Sort::default(), Outcome::default()).await?;
    let page = SquadPage {
        discord_error: Some(error),
        ..SquadPage::new(nav, access, roster)
    };
    Ok((StatusCode::UNPROCESSABLE_ENTITY, page).into_response())
}

async fn remove_webhook(
    State(state): State<AppState>,
    CanConfigure(access): CanConfigure,
) -> Result<Redirect> {
    let squad = &access.squad;
    db::set_webhook(&state.db, &squad.id, None).await?;
    Ok(Redirect::to(&squad_path(&squad.id)))
}

/// The filter and sort a roster form was posted from, to come back to.
#[derive(Deserialize, Default)]
struct ViewForm {
    #[serde(default)]
    filter: Option<String>,
    #[serde(default)]
    sort: Option<String>,
}

#[derive(Deserialize)]
struct AddForm {
    character: String,
    #[serde(default)]
    tags: String,
    #[serde(flatten)]
    view: ViewForm,
}

async fn add_character(
    State(state): State<AppState>,
    CanEdit(access): CanEdit,
    htmx: Htmx,
    nav: Nav,
    Form(form): Form<AddForm>,
) -> Result<Response> {
    let squad = &access.squad;
    let parsed = form
        .character
        .parse::<CharacterName>()
        .map_err(|e| e.to_string())
        .and_then(|c| Ok((c, parse_tags(&form.tags).map_err(|e| e.to_string())?)));
    let error = match parsed {
        Ok((character, tags)) => {
            if db::add_character(&state.db, &squad.id, squad.region, &character, &tags).await? {
                state.wake.notify_one();
                if let Some(hook) = squad.webhook() {
                    let link = state.discord.squad_link(&squad.id);
                    let who = format!("{} {}", character.name(), character.surname());
                    let message = zesquad_core::discord::joined(&who, &squad.name, link.as_deref());
                    state.discord.send(hook, message);
                }
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
    respond(&state, htmx, &nav, access, &form.view, outcome).await
}

#[derive(Deserialize)]
struct TagsForm {
    #[serde(default)]
    tags: String,
    #[serde(flatten)]
    view: ViewForm,
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
    CanEdit(access): CanEdit,
    Path(path): Path<CharacterPath>,
    htmx: Htmx,
    nav: Nav,
    Form(form): Form<TagsForm>,
) -> Result<Response> {
    let squad = &access.squad;
    let outcome = match parse_tags(&form.tags) {
        Ok(tags) => {
            db::add_tags(&state.db, &squad.id, path.id, &tags).await?;
            Outcome::default()
        }
        Err(e) => Outcome::error(e.to_string()),
    };
    respond(&state, htmx, &nav, access, &form.view, outcome).await
}

#[derive(Deserialize)]
struct FilterForm {
    #[serde(flatten)]
    view: ViewForm,
}

async fn remove_tag(
    State(state): State<AppState>,
    CanEdit(access): CanEdit,
    Path(path): Path<CharacterTagPath>,
    htmx: Htmx,
    nav: Nav,
    Form(form): Form<FilterForm>,
) -> Result<Response> {
    let squad = &access.squad;
    db::remove_tag(&state.db, &squad.id, path.id, &path.tag).await?;
    respond(&state, htmx, &nav, access, &form.view, Outcome::default()).await
}

#[derive(Deserialize)]
struct ColourForm {
    #[serde(default)]
    colour: String,
    /// Set by the reset button: back to the default colour.
    #[serde(default)]
    reset: Option<String>,
    #[serde(flatten)]
    view: ViewForm,
}

async fn set_colour(
    State(state): State<AppState>,
    CanEdit(access): CanEdit,
    Path(path): Path<TagPath>,
    htmx: Htmx,
    nav: Nav,
    Form(form): Form<ColourForm>,
) -> Result<Response> {
    let squad = &access.squad;
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
        &nav,
        access,
        &form.view,
        Outcome {
            colours_open: true,
            ..outcome
        },
    )
    .await
}

async fn remove_character(
    State(state): State<AppState>,
    CanEdit(access): CanEdit,
    Path(path): Path<CharacterPath>,
    htmx: Htmx,
    nav: Nav,
    Form(form): Form<FilterForm>,
) -> Result<Response> {
    let squad = &access.squad;
    db::remove_character(&state.db, &squad.id, path.id).await?;
    respond(&state, htmx, &nav, access, &form.view, Outcome::default()).await
}

async fn refresh(
    State(state): State<AppState>,
    CanEdit(access): CanEdit,
    Path(path): Path<CharacterPath>,
    htmx: Htmx,
    nav: Nav,
    Form(form): Form<FilterForm>,
) -> Result<Response> {
    let squad = &access.squad;
    db::request_refresh(&state.db, &squad.id, path.id).await?;
    state.wake.notify_one();
    respond(&state, htmx, &nav, access, &form.view, Outcome::default()).await
}

// ---- auction prices ----

#[derive(Template, WebTemplate)]
#[template(path = "auctions.html")]
struct AuctionsPage {
    nav: Nav,
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

async fn auctions_page(State(state): State<AppState>, nav: Nav) -> Result<AuctionsPage> {
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
        nav,
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
