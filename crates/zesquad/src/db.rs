//! SQLite: opening the database and every query zesquad runs.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::str::FromStr as _;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::Context as _;
use serde_json::Value;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions};
use zesquad_core::access::{Role, Visibility};
use zesquad_core::armory::{Documents, Endpoint, Profile};
use zesquad_core::auctions;
use zesquad_core::discord::Webhook;
use zesquad_core::refresh::{Status, is_due};
use zesquad_core::tags::Colour;
use zesquad_core::{CharacterName, Region, SquadName, Tag};

/// Opens (creating if needed) the database and applies the migrations. Squads from before
/// squads had regions, with no characters to tell, get `default_region`.
///
/// # Errors
///
/// When the file can't be opened or a migration fails.
pub async fn open(path: &Path, default_region: Region) -> anyhow::Result<SqlitePool> {
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    let options = SqliteConnectOptions::from_str("sqlite:")?
        .filename(path)
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .foreign_keys(true)
        .busy_timeout(Duration::from_secs(5));
    let pool = SqlitePoolOptions::new()
        .connect_with(options)
        .await
        .with_context(|| format!("opening {}", path.display()))?;
    sqlx::migrate!()
        .run(&pool)
        .await
        .context("migrating the database")?;
    sqlx::query("UPDATE squads SET region = ? WHERE region IS NULL")
        .bind(default_region.as_str())
        .execute(&pool)
        .await?;
    Ok(pool)
}

/// Seconds since the Unix epoch.
#[must_use]
pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

#[derive(Debug, sqlx::FromRow)]
pub struct Squad {
    pub id: String,
    pub name: String,
    #[sqlx(try_from = "String")]
    pub region: Region,
    /// The Discord webhook's path, as stored; see [`Squad::webhook`].
    discord_webhook: Option<String>,
    #[sqlx(try_from = "String")]
    pub visibility: Visibility,
    /// The secret in its `/join/<token>` link; `None` for squads nobody owns yet.
    pub invite_token: Option<String>,
}

impl Squad {
    /// Where the squad's news goes on Discord, if anywhere.
    #[must_use]
    pub fn webhook(&self) -> Option<Webhook> {
        self.discord_webhook
            .as_deref()
            .and_then(|path| Webhook::from_path(path).ok())
    }
}

/// Sets (or with `None` stops) where the squad's news goes on Discord.
///
/// # Errors
///
/// On database errors.
pub async fn set_webhook(
    db: &SqlitePool,
    squad_id: &str,
    webhook: Option<&Webhook>,
) -> anyhow::Result<()> {
    sqlx::query("UPDATE squads SET discord_webhook = ? WHERE id = ?")
        .bind(webhook.map(Webhook::path))
        .bind(squad_id)
        .execute(db)
        .await?;
    Ok(())
}

/// A squad with a Discord channel that has a given character.
pub struct DiscordSquad {
    pub squad_id: String,
    pub squad_name: String,
    /// The character as the squad shows it, "Name Surname".
    pub character: String,
    pub webhook: Webhook,
}

/// The squads with a Discord channel that have this character.
///
/// # Errors
///
/// On database errors.
pub async fn discord_squads_with(
    db: &SqlitePool,
    region: Region,
    key: &str,
) -> anyhow::Result<Vec<DiscordSquad>> {
    let rows: Vec<(String, String, String, String, String)> = sqlx::query_as(
        "SELECT s.id, s.name, c.name, c.surname, s.discord_webhook
         FROM characters c JOIN squads s ON s.id = c.squad_id
         WHERE c.region = ? AND c.char_key = ? AND s.discord_webhook IS NOT NULL
         ORDER BY s.id",
    )
    .bind(region.as_str())
    .bind(key)
    .fetch_all(db)
    .await?;
    Ok(rows
        .into_iter()
        .filter_map(|(squad_id, squad_name, name, surname, path)| {
            Some(DiscordSquad {
                webhook: Webhook::from_path(&path).ok()?,
                squad_id,
                squad_name,
                character: format!("{name} {surname}"),
            })
        })
        .collect())
}

/// A character's level from its last good Armory summary, if any.
///
/// # Errors
///
/// On database errors.
pub async fn armory_level(db: &SqlitePool, region: &str, key: &str) -> anyhow::Result<Option<u64>> {
    let level: Option<Option<i64>> = sqlx::query_scalar(
        "SELECT json_extract(body, '$.level') FROM armory_documents
         WHERE region = ? AND char_key = ? AND endpoint = ?",
    )
    .bind(region)
    .bind(key)
    .bind(Endpoint::Summary.as_str())
    .fetch_optional(db)
    .await?;
    Ok(level.flatten().and_then(|l| u64::try_from(l).ok()))
}

/// Starts a squad. With an `owner` (accounts on), it's theirs, unlisted, with an invite link;
/// without, it's `open` as squads were before accounts.
///
/// # Errors
///
/// On database errors.
pub async fn create_squad(
    db: &SqlitePool,
    name: &SquadName,
    region: Region,
    owner: Option<i64>,
) -> anyhow::Result<String> {
    let id = zesquad_core::squad_id::encode(uuid::Uuid::new_v4().as_u128());
    let now = now();
    let mut tx = db.begin().await?;
    let (visibility, token) = match owner {
        Some(_) => (Visibility::Unlisted, Some(invite_token())),
        None => (Visibility::Open, None),
    };
    sqlx::query(
        "INSERT INTO squads (id, name, region, created_at, visibility, invite_token)
         VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(name.as_str())
    .bind(region.as_str())
    .bind(now)
    .bind(visibility.as_str())
    .bind(token)
    .execute(&mut *tx)
    .await?;
    if let Some(owner) = owner {
        sqlx::query(
            "INSERT INTO squad_members (squad_id, user_id, role, joined_at) VALUES (?, ?, 'owner', ?)",
        )
        .bind(&id)
        .bind(owner)
        .bind(now)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(id)
}

/// A new invite token: as random as a session token, and as long.
fn invite_token() -> String {
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}

/// # Errors
///
/// On database errors.
pub async fn squad(db: &SqlitePool, id: &str) -> anyhow::Result<Option<Squad>> {
    Ok(sqlx::query_as(
        "SELECT id, name, region, discord_webhook, visibility, invite_token
             FROM squads WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(db)
    .await?)
}

/// `user_id`'s role in a squad; `None` when they aren't in it.
///
/// # Errors
///
/// On database errors.
pub async fn role(db: &SqlitePool, squad_id: &str, user_id: i64) -> anyhow::Result<Option<Role>> {
    let role: Option<String> =
        sqlx::query_scalar("SELECT role FROM squad_members WHERE squad_id = ? AND user_id = ?")
            .bind(squad_id)
            .bind(user_id)
            .fetch_optional(db)
            .await?;
    Ok(role.map(Role::try_from).transpose()?)
}

/// Someone logged in.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct User {
    pub id: i64,
    pub battletag: String,
}

/// Stores who Battle.net says someone is (a new user, or a new BattleTag) and returns them.
///
/// # Errors
///
/// On database errors.
pub async fn upsert_user(
    db: &SqlitePool,
    identity: &zesquad_core::account::Identity,
) -> anyhow::Result<User> {
    let now = now();
    Ok(sqlx::query_as(
        "INSERT INTO users (bnet_id, battletag, created_at, seen_at) VALUES (?, ?, ?, ?)
         ON CONFLICT (bnet_id) DO UPDATE SET battletag = excluded.battletag, seen_at = excluded.seen_at
         RETURNING id, battletag",
    )
    .bind(&identity.id)
    .bind(&identity.battletag)
    .bind(now)
    .bind(now)
    .fetch_one(db)
    .await?)
}

/// Starts a session, and clears out expired ones while at it.
///
/// # Errors
///
/// On database errors.
pub async fn create_session(
    db: &SqlitePool,
    user_id: i64,
    token_hash: &str,
    expires_at: i64,
) -> anyhow::Result<()> {
    let now = now();
    sqlx::query("DELETE FROM sessions WHERE expires_at <= ?")
        .bind(now)
        .execute(db)
        .await?;
    sqlx::query(
        "INSERT INTO sessions (token_hash, user_id, created_at, expires_at) VALUES (?, ?, ?, ?)",
    )
    .bind(token_hash)
    .bind(user_id)
    .bind(now)
    .bind(expires_at)
    .execute(db)
    .await?;
    Ok(())
}

/// The user of a session that hasn't expired, and when it expires.
///
/// # Errors
///
/// On database errors.
pub async fn session_user(
    db: &SqlitePool,
    token_hash: &str,
) -> anyhow::Result<Option<(User, i64)>> {
    let row: Option<(i64, String, i64)> = sqlx::query_as(
        "SELECT u.id, u.battletag, s.expires_at FROM sessions s JOIN users u ON u.id = s.user_id
         WHERE s.token_hash = ? AND s.expires_at > ?",
    )
    .bind(token_hash)
    .bind(now())
    .fetch_optional(db)
    .await?;
    Ok(row.map(|(id, battletag, expires)| (User { id, battletag }, expires)))
}

/// # Errors
///
/// On database errors.
pub async fn extend_session(
    db: &SqlitePool,
    token_hash: &str,
    expires_at: i64,
) -> anyhow::Result<()> {
    sqlx::query("UPDATE sessions SET expires_at = ? WHERE token_hash = ?")
        .bind(expires_at)
        .bind(token_hash)
        .execute(db)
        .await?;
    Ok(())
}

/// When `user_id` first logged in.
///
/// # Errors
///
/// On database errors.
pub async fn user_created(db: &SqlitePool, user_id: i64) -> anyhow::Result<i64> {
    Ok(
        sqlx::query_scalar("SELECT created_at FROM users WHERE id = ?")
            .bind(user_id)
            .fetch_one(db)
            .await?,
    )
}

/// Ends every session of `user_id`.
///
/// # Errors
///
/// On database errors.
pub async fn delete_sessions(db: &SqlitePool, user_id: i64) -> anyhow::Result<()> {
    sqlx::query("DELETE FROM sessions WHERE user_id = ?")
        .bind(user_id)
        .execute(db)
        .await?;
    Ok(())
}

/// Deletes a user and, through the foreign keys, everything that hangs off them.
///
/// # Errors
///
/// On database errors.
pub async fn delete_user(db: &SqlitePool, user_id: i64) -> anyhow::Result<()> {
    sqlx::query("DELETE FROM users WHERE id = ?")
        .bind(user_id)
        .execute(db)
        .await?;
    Ok(())
}

/// # Errors
///
/// On database errors.
pub async fn delete_session(db: &SqlitePool, token_hash: &str) -> anyhow::Result<()> {
    sqlx::query("DELETE FROM sessions WHERE token_hash = ?")
        .bind(token_hash)
        .execute(db)
        .await?;
    Ok(())
}

/// A squad as listed on the front page.
#[derive(Debug, sqlx::FromRow)]
pub struct SquadSummary {
    pub id: String,
    pub name: String,
    pub characters: i64,
}

/// The squads with these ids that exist, in the order given.
///
/// # Errors
///
/// On database errors.
pub async fn squad_summaries(db: &SqlitePool, ids: &[String]) -> anyhow::Result<Vec<SquadSummary>> {
    let mut found: Vec<SquadSummary> = sqlx::query_as(
        "SELECT s.id, s.name, (SELECT COUNT(*) FROM characters c WHERE c.squad_id = s.id) AS characters
         FROM squads s WHERE s.id IN (SELECT value FROM json_each(?))",
    )
    .bind(serde_json::to_string(ids)?)
    .fetch_all(db)
    .await?;
    found.sort_by_key(|s| ids.iter().position(|id| *id == s.id));
    Ok(found)
}

/// # Errors
///
/// On database errors.
pub async fn rename_squad(db: &SqlitePool, id: &str, name: &SquadName) -> anyhow::Result<()> {
    sqlx::query("UPDATE squads SET name = ? WHERE id = ?")
        .bind(name.as_str())
        .bind(id)
        .execute(db)
        .await?;
    Ok(())
}

/// Adds a character to a squad and queues its Armory fetch. `false` if it's already there.
///
/// # Errors
///
/// On database errors.
pub async fn add_character(
    db: &SqlitePool,
    squad_id: &str,
    region: Region,
    character: &CharacterName,
    tags: &[Tag],
) -> anyhow::Result<bool> {
    let mut tx = db.begin().await?;
    let inserted = sqlx::query(
        "INSERT INTO characters (squad_id, region, name, surname, char_key, added_at)
         VALUES (?, ?, ?, ?, ?, ?) ON CONFLICT DO NOTHING",
    )
    .bind(squad_id)
    .bind(region.as_str())
    .bind(character.name())
    .bind(character.surname())
    .bind(character.key())
    .bind(now())
    .execute(&mut *tx)
    .await?;
    if inserted.rows_affected() == 0 {
        return Ok(false);
    }
    let id = inserted.last_insert_rowid();
    for tag in tags {
        insert_tag(&mut tx, id, tag).await?;
    }
    sqlx::query("INSERT INTO armory (region, char_key) VALUES (?, ?) ON CONFLICT DO NOTHING")
        .bind(region.as_str())
        .bind(character.key())
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(true)
}

async fn insert_tag(
    tx: &mut sqlx::SqliteConnection,
    character_id: i64,
    tag: &Tag,
) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO character_tags (character_id, tag_key, label) VALUES (?, ?, ?)
         ON CONFLICT DO NOTHING",
    )
    .bind(character_id)
    .bind(tag.key())
    .bind(tag.label())
    .execute(tx)
    .await?;
    Ok(())
}

/// # Errors
///
/// On database errors.
pub async fn remove_character(db: &SqlitePool, squad_id: &str, id: i64) -> anyhow::Result<()> {
    sqlx::query("DELETE FROM characters WHERE squad_id = ? AND id = ?")
        .bind(squad_id)
        .bind(id)
        .execute(db)
        .await?;
    Ok(())
}

/// Tags a character, if it's in the squad.
///
/// # Errors
///
/// On database errors.
pub async fn add_tags(
    db: &SqlitePool,
    squad_id: &str,
    id: i64,
    tags: &[Tag],
) -> anyhow::Result<()> {
    let mut tx = db.begin().await?;
    let in_squad: Option<i64> =
        sqlx::query_scalar("SELECT id FROM characters WHERE squad_id = ? AND id = ?")
            .bind(squad_id)
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?;
    if in_squad.is_some() {
        for tag in tags {
            insert_tag(&mut tx, id, tag).await?;
        }
    }
    tx.commit().await?;
    Ok(())
}

/// # Errors
///
/// On database errors.
pub async fn remove_tag(
    db: &SqlitePool,
    squad_id: &str,
    id: i64,
    tag_key: &str,
) -> anyhow::Result<()> {
    sqlx::query(
        "DELETE FROM character_tags WHERE tag_key = ? AND character_id =
           (SELECT id FROM characters WHERE squad_id = ? AND id = ?)",
    )
    .bind(tag_key)
    .bind(squad_id)
    .bind(id)
    .execute(db)
    .await?;
    Ok(())
}

/// Marks a squad member's Armory data for fetching as soon as possible.
///
/// # Errors
///
/// On database errors.
pub async fn request_refresh(db: &SqlitePool, squad_id: &str, id: i64) -> anyhow::Result<()> {
    sqlx::query(
        "UPDATE armory SET status = 'pending' WHERE (region, char_key) =
           (SELECT region, char_key FROM characters WHERE squad_id = ? AND id = ?)",
    )
    .bind(squad_id)
    .bind(id)
    .execute(db)
    .await?;
    Ok(())
}

/// A squad member with tags and whatever the Armory said.
#[derive(Debug)]
pub struct Member {
    pub id: i64,
    pub name: String,
    pub surname: String,
    /// `(label, key)`.
    pub tags: Vec<(String, String)>,
    pub status: Status,
    pub fetched_at: Option<i64>,
    pub armory: Armory,
}

/// What the stored Armory documents of a character add up to. Cheap to clone.
#[derive(Debug, Default, Clone)]
pub struct Armory {
    pub profile: Arc<Profile>,
    /// The Armory has answered for this character (kept through a failed refresh).
    pub answered: bool,
}

/// Parsed Armory documents by character, with the `fetched_at` and status they were parsed
/// for, so a roster render (every 3s while polling) only reads and parses documents stored
/// since the last one. One entry per character ever shown; a character is a few KB parsed.
#[derive(Default)]
pub struct ArmoryCache(Mutex<HashMap<(String, String), (Version, Armory)>>);

/// `fetched_at` is in seconds; the status tells apart two fetches in the same second that
/// stored different results (found, then not found).
type Version = (i64, Option<String>);

impl ArmoryCache {
    fn get(&self, key: &(String, String), version: &Version) -> Option<Armory> {
        let cache = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        cache
            .get(key)
            .filter(|(v, _)| v == version)
            .map(|(_, armory)| armory.clone())
    }

    fn put(&self, key: (String, String), version: Version, armory: Armory) {
        let mut cache = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        cache.insert(key, (version, armory));
    }
}

#[derive(sqlx::FromRow)]
struct MemberRow {
    id: i64,
    name: String,
    surname: String,
    region: String,
    char_key: String,
    status: Option<String>,
    fetched_at: Option<i64>,
}

/// The squad's members in the order they were added. Filtering by tag happens after, since
/// some tags come from the Armory data.
///
/// # Errors
///
/// On database errors.
pub async fn members(
    db: &SqlitePool,
    cache: &ArmoryCache,
    squad_id: &str,
) -> anyhow::Result<Vec<Member>> {
    let rows: Vec<MemberRow> = sqlx::query_as(
        "SELECT c.id, c.name, c.surname, c.region, c.char_key, a.status, a.fetched_at
         FROM characters c LEFT JOIN armory a USING (region, char_key)
         WHERE c.squad_id = ? ORDER BY c.added_at, c.id",
    )
    .bind(squad_id)
    .fetch_all(db)
    .await?;

    let tags: Vec<(i64, String, String)> = sqlx::query_as(
        "SELECT t.character_id, t.label, t.tag_key FROM character_tags t
         JOIN characters c ON c.id = t.character_id WHERE c.squad_id = ? ORDER BY t.tag_key",
    )
    .bind(squad_id)
    .fetch_all(db)
    .await?;
    let mut tags_by_member: BTreeMap<i64, Vec<(String, String)>> = BTreeMap::new();
    for (id, label, key) in tags {
        tags_by_member.entry(id).or_default().push((label, key));
    }

    let mut members = Vec::with_capacity(rows.len());
    for r in rows {
        let armory = match r.fetched_at {
            // Never fetched, so nothing stored.
            None => Armory::default(),
            Some(at) => {
                let key = (r.region, r.char_key);
                let version = (at, r.status.clone());
                if let Some(armory) = cache.get(&key, &version) {
                    armory
                } else {
                    let armory = load_armory(db, &key.0, &key.1).await?;
                    cache.put(key, version, armory.clone());
                    armory
                }
            }
        };
        members.push(Member {
            tags: tags_by_member.remove(&r.id).unwrap_or_default(),
            armory,
            status: Status::parse(r.status.as_deref().unwrap_or_default()),
            id: r.id,
            name: r.name,
            surname: r.surname,
            fetched_at: r.fetched_at,
        });
    }
    Ok(members)
}

/// Reads and parses one character's stored Armory documents.
async fn load_armory(db: &SqlitePool, region: &str, key: &str) -> anyhow::Result<Armory> {
    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT endpoint, body FROM armory_documents WHERE region = ? AND char_key = ?",
    )
    .bind(region)
    .bind(key)
    .fetch_all(db)
    .await?;
    let mut docs = Documents::default();
    for (endpoint, body) in rows {
        if let (Some(endpoint), Ok(body)) =
            (Endpoint::parse(&endpoint), serde_json::from_str(&body))
        {
            docs.set(endpoint, body);
        }
    }
    Ok(Armory {
        profile: Arc::new(Profile::from_documents(&docs)),
        answered: docs.summary.is_some(),
    })
}

/// The colours picked for the squad's tags, by tag key. Rows that don't parse (there are none
/// unless someone edited the database) are left out.
///
/// # Errors
///
/// On database errors.
pub async fn tag_colours(
    db: &SqlitePool,
    squad_id: &str,
) -> anyhow::Result<HashMap<String, Colour>> {
    let rows: Vec<(String, String)> =
        sqlx::query_as("SELECT tag_key, colour FROM tag_colours WHERE squad_id = ?")
            .bind(squad_id)
            .fetch_all(db)
            .await?;
    Ok(rows
        .into_iter()
        .filter_map(|(key, colour)| Some((key, Colour::parse(&colour).ok()?)))
        .collect())
}

/// Picks a tag's colour in a squad, or with `None` goes back to its default.
///
/// # Errors
///
/// On database errors.
pub async fn set_tag_colour(
    db: &SqlitePool,
    squad_id: &str,
    tag: &Tag,
    colour: Option<&Colour>,
) -> anyhow::Result<()> {
    if let Some(colour) = colour {
        sqlx::query(
            "INSERT INTO tag_colours (squad_id, tag_key, colour) VALUES (?, ?, ?)
             ON CONFLICT (squad_id, tag_key) DO UPDATE SET colour = excluded.colour",
        )
        .bind(squad_id)
        .bind(tag.key())
        .bind(colour.as_str())
        .execute(db)
        .await?;
    } else {
        sqlx::query("DELETE FROM tag_colours WHERE squad_id = ? AND tag_key = ?")
            .bind(squad_id)
            .bind(tag.key())
            .execute(db)
            .await?;
    }
    Ok(())
}

/// Characters (`region`, `char_key`) in at least one squad whose Armory data is due. Rows with
/// a region this version doesn't know are skipped.
///
/// # Errors
///
/// On database errors.
pub async fn due(
    db: &SqlitePool,
    now: i64,
    interval_secs: i64,
) -> anyhow::Result<Vec<(Region, String)>> {
    let rows: Vec<(String, String, String, Option<i64>)> = sqlx::query_as(
        "SELECT a.region, a.char_key, a.status, a.fetched_at FROM armory a
         WHERE EXISTS (SELECT 1 FROM characters c WHERE (c.region, c.char_key) = (a.region, a.char_key))
         ORDER BY a.fetched_at IS NOT NULL, a.fetched_at",
    )
    .fetch_all(db)
    .await?;
    Ok(rows
        .into_iter()
        .filter(|(_, _, status, fetched_at)| {
            is_due(Status::parse(status), *fetched_at, now, interval_secs)
        })
        .filter_map(|(region, key, _, _)| Some((Region::try_from(region).ok()?, key)))
        .collect())
}

/// The outcome of fetching one character.
pub enum Fetched {
    Found(Vec<(Endpoint, Value)>),
    NotFound,
    Failed(String),
}

/// Records a fetch. A failure keeps the documents from the last good fetch.
///
/// # Errors
///
/// On database errors.
pub async fn store(
    db: &SqlitePool,
    region: &str,
    key: &str,
    fetched: Fetched,
) -> anyhow::Result<()> {
    let mut tx = db.begin().await?;
    let (status, error) = match &fetched {
        Fetched::Found(_) => (Status::Found, None),
        Fetched::NotFound => (Status::NotFound, None),
        Fetched::Failed(e) => (Status::Failed, Some(e.as_str())),
    };
    sqlx::query(
        "UPDATE armory SET status = ?, fetched_at = ?, error = ? WHERE region = ? AND char_key = ?",
    )
    .bind(status.as_str())
    .bind(now())
    .bind(error)
    .bind(region)
    .bind(key)
    .execute(&mut *tx)
    .await?;
    if !matches!(fetched, Fetched::Failed(_)) {
        sqlx::query("DELETE FROM armory_documents WHERE region = ? AND char_key = ?")
            .bind(region)
            .bind(key)
            .execute(&mut *tx)
            .await?;
    }
    if let Fetched::Found(docs) = fetched {
        for (endpoint, body) in docs {
            sqlx::query("INSERT INTO armory_documents (region, char_key, endpoint, body) VALUES (?, ?, ?, ?)")
                .bind(region)
                .bind(key)
                .bind(endpoint.as_str())
                .bind(body.to_string())
                .execute(&mut *tx)
                .await?;
        }
    }
    tx.commit().await?;
    Ok(())
}

// ---- auctions ----

/// Copper and counts are `u64` in core; SQLite stores `i64`.
fn int(n: u64) -> i64 {
    i64::try_from(n).unwrap_or(i64::MAX)
}

fn uint(n: i64) -> u64 {
    u64::try_from(n).unwrap_or(0)
}

/// When a house was last asked for; `None` if never.
///
/// # Errors
///
/// On database errors.
pub async fn auctions_checked_at(
    db: &SqlitePool,
    region: Region,
    house: u32,
) -> anyhow::Result<Option<i64>> {
    let at: Option<(Option<i64>,)> =
        sqlx::query_as("SELECT checked_at FROM auction_houses WHERE region = ? AND house = ?")
            .bind(region.as_str())
            .bind(house)
            .fetch_optional(db)
            .await?;
    Ok(at.and_then(|(at,)| at))
}

/// Stores a house's snapshot taken at `at`, unless it has the same auctions as the last one
/// stored (Battle.net hasn't updated the dump yet). `true` when stored.
///
/// # Errors
///
/// On database errors.
pub async fn store_auctions(
    db: &SqlitePool,
    region: Region,
    house: u32,
    at: i64,
    snapshot: &auctions::Snapshot,
) -> anyhow::Result<bool> {
    let fingerprint = format!("{:016x}", snapshot.fingerprint);
    let mut tx = db.begin().await?;
    let last: Option<(Option<String>,)> =
        sqlx::query_as("SELECT fingerprint FROM auction_houses WHERE region = ? AND house = ?")
            .bind(region.as_str())
            .bind(house)
            .fetch_optional(&mut *tx)
            .await?;
    let changed = last.and_then(|(f,)| f).as_deref() != Some(fingerprint.as_str());
    if changed {
        for p in &snapshot.items {
            sqlx::query(
                "INSERT OR REPLACE INTO auction_snapshots
                 (region, house, at, item, min_price, median_price, quantity, auctions)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(region.as_str())
            .bind(house)
            .bind(at)
            .bind(p.item)
            .bind(int(p.min_price))
            .bind(int(p.median_price))
            .bind(int(p.quantity))
            .bind(int(p.auctions))
            .execute(&mut *tx)
            .await?;
        }
    }
    if changed {
        sqlx::query(
            "INSERT OR REPLACE INTO auction_snapshot_times (region, house, at, items) VALUES (?, ?, ?, ?)",
        )
        .bind(region.as_str())
        .bind(house)
        .bind(at)
        .bind(i64::try_from(snapshot.items.len()).unwrap_or(i64::MAX))
        .execute(&mut *tx)
        .await?;
    }
    sqlx::query(
        "INSERT INTO auction_houses (region, house, checked_at, fingerprint) VALUES (?, ?, ?, ?)
         ON CONFLICT (region, house) DO UPDATE
         SET checked_at = excluded.checked_at, fingerprint = excluded.fingerprint, error = NULL",
    )
    .bind(region.as_str())
    .bind(house)
    .bind(at)
    .bind(&fingerprint)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(changed)
}

/// The `Last-Modified` of a house's items and commodities dumps in its last stored snapshot.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DumpsModified {
    pub items: Option<String>,
    pub commodities: Option<String>,
}

/// See [`DumpsModified`]; both `None` for a house never stored.
///
/// # Errors
///
/// On database errors.
pub async fn auctions_modified(
    db: &SqlitePool,
    region: Region,
    house: u32,
) -> anyhow::Result<DumpsModified> {
    let row: Option<(Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT items_modified, commodities_modified FROM auction_houses
         WHERE region = ? AND house = ?",
    )
    .bind(region.as_str())
    .bind(house)
    .fetch_optional(db)
    .await?;
    Ok(
        row.map_or_else(DumpsModified::default, |(items, commodities)| {
            DumpsModified { items, commodities }
        }),
    )
}

/// Remembers the `Last-Modified` of the dumps just stored (after [`store_auctions`]).
///
/// # Errors
///
/// On database errors.
pub async fn set_auctions_modified(
    db: &SqlitePool,
    region: Region,
    house: u32,
    modified: &DumpsModified,
) -> anyhow::Result<()> {
    sqlx::query(
        "UPDATE auction_houses SET items_modified = ?, commodities_modified = ?
         WHERE region = ? AND house = ?",
    )
    .bind(modified.items.as_deref())
    .bind(modified.commodities.as_deref())
    .bind(region.as_str())
    .bind(house)
    .execute(db)
    .await?;
    Ok(())
}

/// Records a check that found both dumps unchanged since the last snapshot (Battle.net answered
/// 304): nothing new to store.
///
/// # Errors
///
/// On database errors.
pub async fn auctions_unchanged(
    db: &SqlitePool,
    region: Region,
    house: u32,
    at: i64,
) -> anyhow::Result<()> {
    sqlx::query(
        "UPDATE auction_houses SET checked_at = ?, error = NULL WHERE region = ? AND house = ?",
    )
    .bind(at)
    .bind(region.as_str())
    .bind(house)
    .execute(db)
    .await?;
    Ok(())
}

/// Records a failed check; the snapshots so far stay.
///
/// # Errors
///
/// On database errors.
pub async fn auctions_failed(
    db: &SqlitePool,
    region: Region,
    house: u32,
    at: i64,
    error: &str,
) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO auction_houses (region, house, checked_at, error) VALUES (?, ?, ?, ?)
         ON CONFLICT (region, house) DO UPDATE
         SET checked_at = excluded.checked_at, error = excluded.error",
    )
    .bind(region.as_str())
    .bind(house)
    .bind(at)
    .bind(error)
    .execute(db)
    .await?;
    Ok(())
}

/// The latest snapshot of a house: when it was taken and its prices by item.
///
/// # Errors
///
/// On database errors.
pub async fn latest_auctions(
    db: &SqlitePool,
    region: Region,
    house: u32,
) -> anyhow::Result<Option<(i64, Vec<auctions::ItemPrices>)>> {
    let at: Option<(i64,)> = sqlx::query_as(
        "SELECT MAX(at) FROM auction_snapshots WHERE region = ? AND house = ? HAVING COUNT(*) > 0",
    )
    .bind(region.as_str())
    .bind(house)
    .fetch_optional(db)
    .await?;
    let Some((at,)) = at else {
        return Ok(None);
    };
    Ok(Some((at, snapshots(db, region, house, at, at + 1).await?)))
}

/// A house's item prices from snapshots in `[from, to)`, by item, then time.
async fn snapshots(
    db: &SqlitePool,
    region: Region,
    house: u32,
    from: i64,
    to: i64,
) -> anyhow::Result<Vec<auctions::ItemPrices>> {
    let rows: Vec<(u32, i64, i64, i64, i64)> = sqlx::query_as(
        "SELECT item, min_price, median_price, quantity, auctions FROM auction_snapshots
         WHERE region = ? AND house = ? AND at >= ? AND at < ? ORDER BY item, at",
    )
    .bind(region.as_str())
    .bind(house)
    .bind(from)
    .bind(to)
    .fetch_all(db)
    .await?;
    Ok(rows
        .into_iter()
        .map(
            |(item, min, median, quantity, auctions)| auctions::ItemPrices {
                item,
                min_price: uint(min),
                median_price: uint(median),
                quantity: uint(quantity),
                auctions: uint(auctions),
            },
        )
        .collect())
}

/// A house's daily prices for `day` (days since the epoch, UTC), by item.
///
/// # Errors
///
/// On database errors.
pub async fn daily_auctions(
    db: &SqlitePool,
    region: Region,
    house: u32,
    day: i64,
) -> anyhow::Result<Vec<auctions::DailyPrices>> {
    let rows: Vec<(u32, i64, i64, i64, i64, i64)> = sqlx::query_as(
        "SELECT item, min_price, median_price, max_price, quantity, snapshots FROM auction_daily
         WHERE region = ? AND house = ? AND day = ? ORDER BY item",
    )
    .bind(region.as_str())
    .bind(house)
    .bind(day)
    .fetch_all(db)
    .await?;
    Ok(rows
        .into_iter()
        .map(
            |(item, min, median, max, quantity, snapshots)| auctions::DailyPrices {
                item,
                min_price: uint(min),
                median_price: uint(median),
                max_price: uint(max),
                quantity: uint(quantity),
                snapshots: uint(snapshots),
            },
        )
        .collect())
}

/// Condenses a house's finished days (before `now`'s, UTC) into daily prices, then deletes
/// snapshots older than `keep_days` whole days. Snapshots are only deleted once their day is
/// condensed, so a long pause loses nothing.
///
/// # Errors
///
/// On database errors.
pub async fn condense_auctions(
    db: &SqlitePool,
    region: Region,
    house: u32,
    now: i64,
    keep_days: i64,
) -> anyhow::Result<()> {
    let today = auctions::day(now);
    let through: Option<(Option<i64>,)> = sqlx::query_as(
        "SELECT condensed_through FROM auction_houses WHERE region = ? AND house = ?",
    )
    .bind(region.as_str())
    .bind(house)
    .fetch_optional(db)
    .await?;
    let mut through = through.and_then(|(t,)| t);
    let from = through.map_or(i64::MIN, |t| (t + 1) * auctions::DAY);
    let first: (Option<i64>,) = sqlx::query_as(
        "SELECT MIN(at) FROM auction_snapshots WHERE region = ? AND house = ? AND at >= ?",
    )
    .bind(region.as_str())
    .bind(house)
    .bind(from)
    .fetch_one(db)
    .await?;
    if let Some(first) = first.0 {
        for day in auctions::day(first)..today {
            let start = day * auctions::DAY;
            let rows = snapshots(db, region, house, start, start + auctions::DAY).await?;
            let mut tx = db.begin().await?;
            for item_rows in rows.chunk_by(|a, b| a.item == b.item) {
                let Some(d) = auctions::daily(item_rows[0].item, item_rows) else {
                    continue;
                };
                sqlx::query(
                    "INSERT OR REPLACE INTO auction_daily
                     (region, house, day, item, min_price, median_price, max_price, quantity, snapshots)
                     VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
                )
                .bind(region.as_str())
                .bind(house)
                .bind(day)
                .bind(d.item)
                .bind(int(d.min_price))
                .bind(int(d.median_price))
                .bind(int(d.max_price))
                .bind(int(d.quantity))
                .bind(int(d.snapshots))
                .execute(&mut *tx)
                .await?;
            }
            sqlx::query(
                "UPDATE auction_houses SET condensed_through = ? WHERE region = ? AND house = ?",
            )
            .bind(day)
            .bind(region.as_str())
            .bind(house)
            .execute(&mut *tx)
            .await?;
            tx.commit().await?;
            through = Some(day);
        }
    }
    if let Some(through) = through {
        let keep_from = (today - keep_days).min(through + 1) * auctions::DAY;
        sqlx::query("DELETE FROM auction_snapshots WHERE region = ? AND house = ? AND at < ?")
            .bind(region.as_str())
            .bind(house)
            .bind(keep_from)
            .execute(db)
            .await?;
        sqlx::query("DELETE FROM auction_snapshot_times WHERE region = ? AND house = ? AND at < ?")
            .bind(region.as_str())
            .bind(house)
            .bind(keep_from)
            .execute(db)
            .await?;
    }
    Ok(())
}

/// How collecting a house's prices is going.
#[derive(Debug, Default)]
pub struct HouseStatus {
    /// Last time it was asked for.
    pub checked_at: Option<i64>,
    /// Why the last check failed.
    pub error: Option<String>,
    /// Snapshots kept (about a week's).
    pub snapshots: i64,
    pub first_at: Option<i64>,
    pub last_at: Option<i64>,
    /// Items in the latest snapshot.
    pub last_items: i64,
}

/// # Errors
///
/// On database errors.
pub async fn house_status(
    db: &SqlitePool,
    region: Region,
    house: u32,
) -> anyhow::Result<HouseStatus> {
    let checked: Option<(Option<i64>, Option<String>)> = sqlx::query_as(
        "SELECT checked_at, error FROM auction_houses WHERE region = ? AND house = ?",
    )
    .bind(region.as_str())
    .bind(house)
    .fetch_optional(db)
    .await?;
    let (snapshots, first_at, last_at): (i64, Option<i64>, Option<i64>) = sqlx::query_as(
        "SELECT COUNT(*), MIN(at), MAX(at) FROM auction_snapshot_times WHERE region = ? AND house = ?",
    )
    .bind(region.as_str())
    .bind(house)
    .fetch_one(db)
    .await?;
    let last_items: Option<(i64,)> = sqlx::query_as(
        "SELECT items FROM auction_snapshot_times WHERE region = ? AND house = ? AND at = ?",
    )
    .bind(region.as_str())
    .bind(house)
    .bind(last_at)
    .fetch_optional(db)
    .await?;
    let (checked_at, error) = checked.unwrap_or_default();
    Ok(HouseStatus {
        checked_at,
        error,
        snapshots,
        first_at,
        last_at,
        last_items: last_items.map_or(0, |(n,)| n),
    })
}

/// An item's typical price (each snapshot's median) over a stretch of time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PriceRange {
    pub item: u32,
    pub min: u64,
    pub avg: u64,
    pub max: u64,
    /// Snapshots that had the item.
    pub snapshots: u64,
}

/// The price ranges of `items` in a house's snapshots since `since`, for the items that were on
/// sale. Looks each item up in each snapshot by key, so it stays quick with a week of data.
///
/// # Errors
///
/// On database errors.
pub async fn price_ranges(
    db: &SqlitePool,
    region: Region,
    house: u32,
    items: &[u32],
    since: i64,
) -> anyhow::Result<Vec<PriceRange>> {
    if items.is_empty() {
        return Ok(Vec::new());
    }
    let items = serde_json::to_string(items)?;
    let rows: Vec<(u32, i64, i64, i64, i64)> = sqlx::query_as(
        "SELECT s.item, MIN(s.median_price), CAST(ROUND(AVG(s.median_price)) AS INTEGER),
                MAX(s.median_price), COUNT(*)
         FROM auction_snapshot_times t
         JOIN auction_snapshots s
           ON s.region = t.region AND s.house = t.house AND s.at = t.at
          AND s.item IN (SELECT value FROM json_each(?))
         WHERE t.region = ? AND t.house = ? AND t.at >= ?
         GROUP BY s.item",
    )
    .bind(items)
    .bind(region.as_str())
    .bind(house)
    .bind(since)
    .fetch_all(db)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(item, min, avg, max, snapshots)| PriceRange {
            item,
            min: uint(min),
            avg: uint(avg),
            max: uint(max),
            snapshots: uint(snapshots),
        })
        .collect())
}

/// One snapshot's typical price (median) of an item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PricePoint {
    pub item: u32,
    pub at: i64,
    pub median: u64,
}

/// The median price of `items` in each of a house's snapshots since `since`, by item, then
/// time. Looks each item up in each snapshot by key, like [`price_ranges`].
///
/// # Errors
///
/// On database errors.
pub async fn price_series(
    db: &SqlitePool,
    region: Region,
    house: u32,
    items: &[u32],
    since: i64,
) -> anyhow::Result<Vec<PricePoint>> {
    if items.is_empty() {
        return Ok(Vec::new());
    }
    let items = serde_json::to_string(items)?;
    let rows: Vec<(u32, i64, i64)> = sqlx::query_as(
        "SELECT s.item, s.at, s.median_price
         FROM auction_snapshot_times t
         JOIN auction_snapshots s
           ON s.region = t.region AND s.house = t.house AND s.at = t.at
          AND s.item IN (SELECT value FROM json_each(?))
         WHERE t.region = ? AND t.house = ? AND t.at >= ?
         ORDER BY s.item, s.at",
    )
    .bind(items)
    .bind(region.as_str())
    .bind(house)
    .bind(since)
    .fetch_all(db)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(item, at, median)| PricePoint {
            item,
            at,
            median: uint(median),
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use zesquad_core::auctions::{DAY, ItemPrices, Snapshot};

    async fn temp_db() -> (tempfile::TempDir, SqlitePool) {
        let dir = tempfile::tempdir().unwrap();
        let db = open(&dir.path().join("test.db"), Region::Eu).await.unwrap();
        (dir, db)
    }

    fn snapshot(fingerprint: u64, median: u64) -> Snapshot {
        Snapshot {
            items: vec![ItemPrices {
                item: 2589,
                min_price: median / 2,
                median_price: median,
                quantity: 10,
                auctions: 2,
            }],
            fingerprint,
        }
    }

    #[tokio::test]
    async fn unchanged_dumps_are_not_stored_twice() {
        let (_dir, db) = temp_db().await;
        assert!(
            store_auctions(&db, Region::Eu, 110, 100, &snapshot(1, 10))
                .await
                .unwrap()
        );
        assert!(
            !store_auctions(&db, Region::Eu, 110, 200, &snapshot(1, 10))
                .await
                .unwrap()
        );
        assert_eq!(
            auctions_checked_at(&db, Region::Eu, 110).await.unwrap(),
            Some(200)
        );
        let (at, _) = latest_auctions(&db, Region::Eu, 110)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(at, 100, "the repeat isn't a new snapshot");
        assert!(
            store_auctions(&db, Region::Eu, 110, 300, &snapshot(2, 10))
                .await
                .unwrap()
        );
        // Same auctions in another house or region are that house's own.
        assert!(
            store_auctions(&db, Region::Us, 110, 300, &snapshot(2, 10))
                .await
                .unwrap()
        );
        assert_eq!(latest_auctions(&db, Region::Eu, 111).await.unwrap(), None);
    }

    #[tokio::test]
    async fn failures_keep_the_snapshots() {
        let (_dir, db) = temp_db().await;
        store_auctions(&db, Region::Eu, 110, 100, &snapshot(1, 10))
            .await
            .unwrap();
        auctions_failed(&db, Region::Eu, 110, 200, "503")
            .await
            .unwrap();
        assert_eq!(
            auctions_checked_at(&db, Region::Eu, 110).await.unwrap(),
            Some(200)
        );
        assert!(
            latest_auctions(&db, Region::Eu, 110)
                .await
                .unwrap()
                .is_some()
        );
        // The fingerprint survives a failure, so the same dump afterwards isn't stored again.
        assert!(
            !store_auctions(&db, Region::Eu, 110, 300, &snapshot(1, 10))
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn finished_days_are_condensed_and_old_snapshots_pruned() {
        let (_dir, db) = temp_db().await;
        let first_day = 20_000 * DAY;
        // Day 0: three snapshots; day 1: one.
        for (i, (at, median)) in [
            (first_day, 100),
            (first_day + 3600, 140),
            (first_day + 7200, 120),
            (first_day + DAY, 90),
        ]
        .into_iter()
        .enumerate()
        {
            store_auctions(&db, Region::Eu, 110, at, &snapshot(i as u64, median))
                .await
                .unwrap();
        }

        // During day 1, only day 0 is finished.
        condense_auctions(&db, Region::Eu, 110, first_day + DAY + 60, 7)
            .await
            .unwrap();
        let days = daily_auctions(&db, Region::Eu, 110, 20_000).await.unwrap();
        assert_eq!(days.len(), 1);
        assert_eq!(
            (
                days[0].min_price,
                days[0].median_price,
                days[0].max_price,
                days[0].snapshots
            ),
            (50, 120, 140, 3)
        );
        assert_eq!(
            daily_auctions(&db, Region::Eu, 110, 20_001).await.unwrap(),
            []
        );

        // A week on, day 0's snapshots go; day 1 is condensed on the way, and its snapshots
        // stay while they're within the week.
        condense_auctions(&db, Region::Eu, 110, first_day + 8 * DAY, 7)
            .await
            .unwrap();
        assert_eq!(
            daily_auctions(&db, Region::Eu, 110, 20_001)
                .await
                .unwrap()
                .len(),
            1
        );
        let left: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM auction_snapshots")
            .fetch_one(&db)
            .await
            .unwrap();
        assert_eq!(left.0, 1);
        let times: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM auction_snapshot_times")
            .fetch_one(&db)
            .await
            .unwrap();
        assert_eq!(times.0, 1, "snapshot times are pruned with the snapshots");
        assert_eq!(
            daily_auctions(&db, Region::Eu, 110, 20_000)
                .await
                .unwrap()
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn price_ranges_and_status_over_the_snapshots_kept() {
        let (_dir, db) = temp_db().await;
        assert_eq!(
            house_status(&db, Region::Eu, 110).await.unwrap().snapshots,
            0
        );
        for (i, (at, median)) in [(100, 10), (200, 30), (300, 23)].into_iter().enumerate() {
            store_auctions(&db, Region::Eu, 110, at, &snapshot(i as u64, median))
                .await
                .unwrap();
        }
        let ranges = price_ranges(&db, Region::Eu, 110, &[2589, 999], 0)
            .await
            .unwrap();
        assert_eq!(
            ranges,
            [PriceRange {
                item: 2589,
                min: 10,
                avg: 21,
                max: 30,
                snapshots: 3
            }],
            "items not on sale are left out"
        );
        let later = price_ranges(&db, Region::Eu, 110, &[2589], 200)
            .await
            .unwrap();
        assert_eq!((later[0].min, later[0].snapshots), (23, 2));
        let series = price_series(&db, Region::Eu, 110, &[2589, 999], 200)
            .await
            .unwrap();
        assert_eq!(
            series,
            [
                PricePoint {
                    item: 2589,
                    at: 200,
                    median: 30
                },
                PricePoint {
                    item: 2589,
                    at: 300,
                    median: 23
                },
            ]
        );
        assert_eq!(
            price_ranges(&db, Region::Eu, 111, &[2589], 0)
                .await
                .unwrap(),
            []
        );

        let status = house_status(&db, Region::Eu, 110).await.unwrap();
        assert_eq!(
            (
                status.snapshots,
                status.first_at,
                status.last_at,
                status.last_items
            ),
            (3, Some(100), Some(300), 1)
        );
        assert_eq!(status.checked_at, Some(300));
    }

    #[tokio::test]
    async fn snapshots_are_only_pruned_once_condensed() {
        let (_dir, db) = temp_db().await;
        let first_day = 20_000 * DAY;
        store_auctions(&db, Region::Eu, 110, first_day, &snapshot(1, 10))
            .await
            .unwrap();
        // Nothing condensed for this house yet and today is day 0: nothing goes.
        condense_auctions(&db, Region::Eu, 110, first_day + 60, 0)
            .await
            .unwrap();
        assert!(
            latest_auctions(&db, Region::Eu, 110)
                .await
                .unwrap()
                .is_some()
        );
    }
}
