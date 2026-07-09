//! Loader for the legacy quick.db SQLite file (`json.sqlite`).
//!
//! quick.db keeps one row per table (`bot`, `guilds`) whose `json` column
//! holds the whole document; see `docs/specs/data-model-legacy.md` for the
//! shapes this module parses into.

mod model;
#[cfg(test)]
mod tests;

pub use model::*;

use anyhow::{Context, Result};
use sea_orm::sea_query::{Expr, ExprTrait, Query};
use sea_orm::{ConnectOptions, ConnectionTrait, Database, DatabaseConnection};
use std::collections::HashMap;

/// Where the production quick.db file lives relative to the working dir.
pub const DEFAULT_LEGACY_DB_PATH: &str = "./json.sqlite";

/// Fully-typed snapshot of the legacy quick.db documents.
#[derive(Debug)]
pub struct LegacyData {
    pub bot: LegacyBot,
    /// Root guild map, including `/forgetme` tombstones.
    pub guilds: HashMap<String, GuildEntry>,
}

impl LegacyData {
    /// Loads and parses both documents from the quick.db SQLite file at `path`
    /// (a filesystem path, e.g. `./json.sqlite`).
    pub async fn load(path: &str) -> Result<Self> {
        // sqlx statement logging is disabled (it defaults to INFO) so `import`
        // does not interleave raw quick.db SQL with the report operators must
        // review, regardless of `DEBUG`.
        let options = ConnectOptions::new(legacy_url(path)).sqlx_logging(false).to_owned();
        let url = options.get_url().to_owned();
        let db = Database::connect(options)
            .await
            .with_context(|| format!("connecting to legacy quick.db at {url}"))?;

        let bot_json = fetch_json(&db, "bot").await?;
        let guilds_json = fetch_json(&db, "guilds").await?;
        if let Err(err) = db.close().await {
            log::warn!("failed to close legacy quick.db connection: {err}");
        }

        let bot: LegacyBot =
            serde_json::from_str(&bot_json).context("parsing legacy `bot` document")?;
        let guilds = parse_guilds(&guilds_json)?;

        let data = Self { bot, guilds };
        log::info!(
            "loaded legacy data: {} root guild keys ({} tombstones, {} corrupt)",
            data.guilds.len(),
            data.tombstone_count(),
            data.corrupt_count(),
        );
        Ok(data)
    }

    /// Valid guilds only (tombstones skipped), as `(guild_id, guild)` pairs.
    #[cfg(test)]
    pub fn guilds(&self) -> impl Iterator<Item = (&str, &LegacyGuild)> {
        self.guilds
            .iter()
            .filter_map(|(id, entry)| entry.as_guild().map(|guild| (id.as_str(), guild)))
    }

    /// Number of `/forgetme` tombstones at the root of the guilds document.
    pub fn tombstone_count(&self) -> usize {
        self.guilds.values().filter(|entry| entry.is_tombstone()).count()
    }

    /// Number of corrupt (non-object, non-tombstone) root guild entries.
    /// Reported by migration-import.md Phase 0; production has 0.
    pub fn corrupt_count(&self) -> usize {
        self.guilds.values().filter(|entry| entry.is_corrupt()).count()
    }

    /// Historical global counters for the `bot_stats` table: every per-command
    /// counter plus `trophiesAwarded` and `rootTrophies` (ADR 0006 keeps them
    /// as a record only; they are known-unreliable).
    pub fn bot_stats(&self) -> HashMap<String, u64> {
        let mut stats = self.bot.commands.clone();
        stats.insert("trophiesAwarded".to_owned(), self.bot.trophies_awarded);
        stats.insert("rootTrophies".to_owned(), self.bot.trophies);
        stats
    }
}

/// Builds the SQLite connection URL for the legacy quick.db file, enforcing
/// `mode=ro`: `json.sqlite` is strictly read-only input (migration-import.md
/// principle 1). A caller-supplied explicit `mode=` is respected.
fn legacy_url(path: &str) -> String {
    let mut url = if path.starts_with("sqlite:") {
        path.to_owned()
    } else {
        format!("sqlite://{path}")
    };
    if !url.contains("mode=") {
        url.push(if url.contains('?') { '&' } else { '?' });
        url.push_str("mode=ro");
    }
    url
}

/// Parses the root `guilds` document one guild at a time so a failure names
/// the offending guild key instead of drowning in the multi-megabyte blob.
fn parse_guilds(json: &str) -> Result<HashMap<String, GuildEntry>> {
    let raw: HashMap<String, serde_json::Value> =
        serde_json::from_str(json).context("parsing legacy `guilds` document")?;
    raw.into_iter()
        .map(|(id, value)| {
            let entry: GuildEntry = serde_json::from_value(value)
                .with_context(|| format!("parsing legacy guild `{id}`"))?;
            Ok((id, entry))
        })
        .collect()
}

/// Reads the `json` column of a quick.db table's single `ID = 'data'` row.
async fn fetch_json(db: &DatabaseConnection, table: &'static str) -> Result<String> {
    let mut query = Query::select();
    query.from(table).column("json").and_where(Expr::col("ID").eq("data")).limit(1);

    db.query_one(&query)
        .await
        .with_context(|| format!("querying legacy table `{table}`"))?
        .with_context(|| format!("legacy table `{table}` is empty"))?
        .try_get("", "json")
        .with_context(|| format!("reading `json` column of legacy table `{table}`"))
}
