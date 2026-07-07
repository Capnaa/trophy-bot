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
use sea_orm::sea_query::Query;
use sea_orm::{ConnectionTrait, Database, DatabaseConnection};
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
        let url = if path.starts_with("sqlite:") {
            path.to_owned()
        } else {
            format!("sqlite://{path}")
        };
        let db = Database::connect(&url)
            .await
            .with_context(|| format!("connecting to legacy quick.db at {url}"))?;

        let bot_json = fetch_json(&db, "bot").await?;
        let guilds_json = fetch_json(&db, "guilds").await?;
        if let Err(err) = db.close().await {
            log::warn!("failed to close legacy quick.db connection: {err}");
        }

        let bot: LegacyBot =
            serde_json::from_str(&bot_json).context("parsing legacy `bot` document")?;
        let guilds: HashMap<String, GuildEntry> =
            serde_json::from_str(&guilds_json).context("parsing legacy `guilds` document")?;

        log::info!(
            "loaded legacy data: {} root guild keys ({} tombstones)",
            guilds.len(),
            guilds.values().filter(|entry| entry.is_tombstone()).count(),
        );
        Ok(Self { bot, guilds })
    }

    /// [`Self::load`] from the default `./json.sqlite` location.
    pub async fn load_default() -> Result<Self> {
        Self::load(DEFAULT_LEGACY_DB_PATH).await
    }

    /// Valid guilds only (tombstones skipped), as `(guild_id, guild)` pairs.
    pub fn guilds(&self) -> impl Iterator<Item = (&str, &LegacyGuild)> {
        self.guilds
            .iter()
            .filter_map(|(id, entry)| entry.as_guild().map(|guild| (id.as_str(), guild)))
    }

    /// Number of `/forgetme` tombstones at the root of the guilds document.
    pub fn tombstone_count(&self) -> usize {
        self.guilds.values().filter(|entry| entry.is_tombstone()).count()
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

/// Reads the single-row `json` column of a quick.db table.
async fn fetch_json(db: &DatabaseConnection, table: &'static str) -> Result<String> {
    let mut query = Query::select();
    query.from(table).column("json").limit(1);

    db.query_one(&query)
        .await
        .with_context(|| format!("querying legacy table `{table}`"))?
        .with_context(|| format!("legacy table `{table}` is empty"))?
        .try_get("", "json")
        .with_context(|| format!("reading `json` column of legacy table `{table}`"))
}
