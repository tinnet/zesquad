//! SQLite: opening the database and every query zesquad runs.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::str::FromStr as _;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::Context as _;
use serde_json::Value;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions};
use zesquad_core::armory::{Documents, Endpoint, Profile};
use zesquad_core::refresh::{Status, is_due};
use zesquad_core::{CharacterName, SquadName, Tag};

/// Opens (creating if needed) the database and applies the migrations.
///
/// # Errors
///
/// When the file can't be opened or a migration fails.
pub async fn open(path: &Path) -> anyhow::Result<SqlitePool> {
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
}

/// # Errors
///
/// On database errors.
pub async fn create_squad(db: &SqlitePool, name: &SquadName) -> anyhow::Result<String> {
    let id = uuid::Uuid::new_v4().to_string();
    sqlx::query("INSERT INTO squads (id, name, created_at) VALUES (?, ?, ?)")
        .bind(&id)
        .bind(name.as_str())
        .bind(now())
        .execute(db)
        .await?;
    Ok(id)
}

/// # Errors
///
/// On database errors.
pub async fn squad(db: &SqlitePool, id: &str) -> anyhow::Result<Option<Squad>> {
    Ok(sqlx::query_as("SELECT id, name FROM squads WHERE id = ?")
        .bind(id)
        .fetch_optional(db)
        .await?)
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
    region: &str,
    character: &CharacterName,
    tags: &[Tag],
) -> anyhow::Result<bool> {
    let mut tx = db.begin().await?;
    let inserted = sqlx::query(
        "INSERT INTO characters (squad_id, region, name, surname, char_key, added_at)
         VALUES (?, ?, ?, ?, ?, ?) ON CONFLICT DO NOTHING",
    )
    .bind(squad_id)
    .bind(region)
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
        .bind(region)
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
    pub error: Option<String>,
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
    error: Option<String>,
}

/// The squad's members in the order they were added, optionally only those with a tag.
///
/// # Errors
///
/// On database errors.
pub async fn members(
    db: &SqlitePool,
    cache: &ArmoryCache,
    squad_id: &str,
    tag_key: Option<&str>,
) -> anyhow::Result<Vec<Member>> {
    let rows: Vec<MemberRow> = sqlx::query_as(
        "SELECT c.id, c.name, c.surname, c.region, c.char_key, a.status, a.fetched_at, a.error
         FROM characters c LEFT JOIN armory a USING (region, char_key)
         WHERE c.squad_id = ?1 AND (?2 IS NULL OR EXISTS
           (SELECT 1 FROM character_tags t WHERE t.character_id = c.id AND t.tag_key = ?2))
         ORDER BY c.added_at, c.id",
    )
    .bind(squad_id)
    .bind(tag_key)
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
            error: r.error,
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

/// Every tag used in the squad, `(label, key)`, alphabetically.
///
/// # Errors
///
/// On database errors.
pub async fn squad_tags(db: &SqlitePool, squad_id: &str) -> anyhow::Result<Vec<(String, String)>> {
    Ok(sqlx::query_as(
        "SELECT MIN(t.label), t.tag_key FROM character_tags t
         JOIN characters c ON c.id = t.character_id WHERE c.squad_id = ?
         GROUP BY t.tag_key ORDER BY t.tag_key",
    )
    .bind(squad_id)
    .fetch_all(db)
    .await?)
}

/// Characters (`region`, `char_key`) in at least one squad whose Armory data is due.
///
/// # Errors
///
/// On database errors.
pub async fn due(
    db: &SqlitePool,
    now: i64,
    interval_secs: i64,
) -> anyhow::Result<Vec<(String, String)>> {
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
        .map(|(region, key, _, _)| (region, key))
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
