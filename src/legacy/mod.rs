//! Loader for the legacy quick.db SQLite file (`json.sqlite`).
//!
//! quick.db keeps one row per table (`bot`, `guilds`) whose `json` column
//! holds the whole document; see `docs/specs/data-model-legacy.md` for the
//! shapes this module parses into.

mod model;

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

#[cfg(test)]
mod tests {
    //! Legacy loader tests. Fixture-based tests always run; tests marked
    //! `#[ignore]` validate against the REAL production dumps (`bot_db.json`,
    //! `guilds_db.json`, `json.sqlite`), which are git-excluded production data —
    //! run them explicitly with `cargo test -- --ignored` on a machine holding
    //! the snapshot (pre-cutover verification). Expected counts come from
    //! `docs/specs/data-model-legacy.md`.

    use super::*;
    use std::collections::HashMap;
    use std::path::PathBuf;

    fn repo_file(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(name)
    }

    fn load_guilds_dump() -> HashMap<String, GuildEntry> {
        let raw = std::fs::read_to_string(repo_file("guilds_db.json")).expect("read guilds_db.json");
        serde_json::from_str(&raw).expect("parse guilds_db.json")
    }

    #[test]
    #[ignore = "requires the git-excluded production snapshot (bot_db.json)"]
    fn bot_dump_parses_with_expected_counters() {
        let raw = std::fs::read_to_string(repo_file("bot_db.json")).expect("read bot_db.json");
        let bot: LegacyBot = serde_json::from_str(&raw).expect("parse bot_db.json");

        assert_eq!(bot.commands.get("total"), Some(&104_913));
        assert_eq!(bot.commands.get("award"), Some(&41_240));
        assert_eq!(bot.trophies, 10_571);
        assert_eq!(bot.trophies_awarded, 120_411);
    }

    #[test]
    #[ignore = "requires the git-excluded production snapshot (guilds_db.json)"]
    fn guilds_dump_root_entries_split_into_tombstones_and_guilds() {
        let guilds = load_guilds_dump();

        assert_eq!(guilds.len(), 2_493, "root guild keys");
        let tombstones = guilds.values().filter(|e| e.is_tombstone()).count();
        assert_eq!(tombstones, 5, "/forgetme tombstones");
        let valid = guilds.values().filter_map(GuildEntry::as_guild).count();
        assert_eq!(valid, 2_488, "valid guild objects");
    }

    #[test]
    #[ignore = "requires the git-excluded production snapshot (guilds_db.json)"]
    fn guilds_dump_trophy_award_and_config_counts_match_spec() {
        let guilds = load_guilds_dump();
        let valid: Vec<&LegacyGuild> = guilds.values().filter_map(GuildEntry::as_guild).collect();

        let trophies: usize = valid.iter().map(|g| g.trophy_defs().count()).sum();
        assert_eq!(trophies, 10_853, "trophies (skipping the 'current' counter)");

        let awards: usize = valid
            .iter()
            .flat_map(|g| g.users.values())
            .map(|u| u.trophies.len())
            .sum();
        assert_eq!(awards, 60_554, "total award elements");

        let empty_users = valid
            .iter()
            .flat_map(|g| g.users.values())
            .filter(|u| u.trophies.is_empty())
            .count();
        assert_eq!(empty_users, 1_284, "users with empty trophies arrays");

        let panels = valid.iter().filter(|g| g.panel.is_some()).count();
        assert_eq!(panels, 461, "guilds with a leaderboard panel");

        let rewards: usize = valid.iter().map(|g| g.rewards.len()).sum();
        assert_eq!(rewards, 287, "role reward entries");
    }

    #[test]
    fn trophy_map_keeps_current_counter_separate_from_definitions() {
        let json = r#"{
            "current": 3,
            "1": {"name": "First", "value": 10},
            "2": {"name": "Second", "value": 8.5}
        }"#;
        let map: HashMap<String, TrophyEntry> = serde_json::from_str(json).expect("parse trophy map");
        let guild = LegacyGuild { trophies: map, ..Default::default() };

        assert_eq!(guild.trophies.len(), 3, "counter entry is kept in the raw map");
        assert!(
            matches!(guild.trophies.get("current"), Some(TrophyEntry::Counter)),
            "bare integer parses as the counter, not a trophy"
        );
        let mut defs: Vec<(&str, &LegacyTrophy)> = guild.trophy_defs().collect();
        defs.sort_by_key(|(id, _)| *id);
        assert_eq!(defs.len(), 2);
        assert_eq!(defs[0].1.name, "First");
        assert_eq!(defs[1].1.value, 8.5, "non-integer float values must survive");
    }

    #[test]
    fn dedication_tolerates_all_four_legacy_shapes() {
        let shapes = [
            (r#"{"name": "A", "value": 1}"#, None, None),
            (r#"{"name": "B", "value": 1, "dedication": {}}"#, None, None),
            (
                r#"{"name": "C", "value": 1, "dedication": {"user": null, "name": null}}"#,
                None,
                None,
            ),
            (
                r#"{"name": "D", "value": 1, "dedication": {"user": null, "name": "free text"}}"#,
                None,
                Some("free text"),
            ),
            (
                r#"{"name": "E", "value": 1, "dedication": {"user": "123", "name": "someone"}}"#,
                Some("123"),
                Some("someone"),
            ),
        ];
        for (json, user, name) in shapes {
            let trophy: LegacyTrophy = serde_json::from_str(json).expect("parse trophy");
            assert_eq!(trophy.dedication.user.as_deref(), user, "{json}");
            assert_eq!(trophy.dedication.name.as_deref(), name, "{json}");
        }
    }

    #[test]
    #[ignore = "requires the git-excluded production snapshot (guilds_db.json)"]
    fn guilds_dump_float_imsafe_and_image_shape_counts_match_spec() {
        let guilds = load_guilds_dump();
        let valid: Vec<&LegacyGuild> = guilds.values().filter_map(GuildEntry::as_guild).collect();

        let float_trophies = valid
            .iter()
            .flat_map(|g| g.trophy_defs())
            .filter(|(_, t)| t.value.fract() != 0.0)
            .count();
        assert_eq!(float_trophies, 44, "trophies with non-integer float values");

        let guilds_with_float_trophies = valid
            .iter()
            .filter(|g| g.trophy_defs().any(|(_, t)| t.value.fract() != 0.0))
            .count();
        assert_eq!(guilds_with_float_trophies, 19, "guilds owning float-valued trophies");

        let float_users = valid
            .iter()
            .flat_map(|g| g.users.values())
            .filter(|u| u.trophy_value.fract() != 0.0)
            .count();
        assert_eq!(float_users, 60, "users with float trophyValue");

        let imsafe_present = valid.iter().filter(|g| g.imsafe.is_some()).count();
        assert_eq!(imsafe_present, 2_407, "guilds with an imsafe key");
        assert!(
            valid.iter().all(|g| g.imsafe != Some(false)),
            "imsafe is always true when present"
        );

        let (mut null, mut local, mut cdn) = (0usize, 0usize, 0usize);
        for (_, trophy) in valid.iter().flat_map(|g| g.trophy_defs()) {
            match trophy.image.as_deref() {
                None => null += 1,
                Some(image) if image.starts_with("https://cdn.discordapp.com/") => cdn += 1,
                Some(_) => local += 1,
            }
        }
        assert_eq!(
            (null, local, cdn),
            (7_965, 2_693, 195),
            "image shapes (null / local filename / CDN URL)"
        );
    }

    #[test]
    fn guild_entry_tolerates_unknown_keys_and_rejects_unknown_integers() {
        let json = r#"{
            "id": "1", "language": "en", "restapi": {"token": "", "enabled": false},
            "tropies": {"current": 1},
            "imsafe": true, "settings": {"dedication_display": 2},
            "trophies": {"current": 0}, "users": {}, "rewards": [], "permissions": {}
        }"#;
        let entry: GuildEntry = serde_json::from_str(json).expect("guild with unknown keys");
        let guild = entry.as_guild().expect("valid guild");
        assert_eq!(guild.imsafe, Some(true));
        assert_eq!(guild.settings.get("dedication_display"), Some(&2));
        assert_eq!(guild.trophy_defs().count(), 0);

        let tombstone: GuildEntry = serde_json::from_str("-1").expect("tombstone");
        assert!(tombstone.is_tombstone());
    }

    #[test]
    fn guild_entry_classifies_non_object_non_tombstone_values_as_corrupt() {
        // migration-import.md Phase 0: only the literal integer -1 is a tombstone;
        // any other non-object guild value must surface as corrupt (0 expected in
        // production) instead of hard-failing the whole document parse.
        for json in ["7", "-1.0", "2.5", "\"broken\"", "true", "null", "[]"] {
            let entry: GuildEntry = serde_json::from_str(json).expect(json);
            assert!(entry.is_corrupt(), "{json} should be corrupt");
            assert!(!entry.is_tombstone(), "{json} is not a tombstone");
            assert!(entry.as_guild().is_none(), "{json} is not a guild");
        }
    }

    #[test]
    fn trophy_parse_errors_name_the_trophy_id_and_real_cause() {
        // A trophy missing its required `name` field must not be swallowed into an
        // opaque "did not match any variant" error.
        let json = r#"{"trophies": {"current": 2, "1": {"value": 10}}}"#;
        let err = serde_json::from_str::<LegacyGuild>(json).expect_err("missing name").to_string();
        assert!(err.contains("trophy `1`"), "error should name the trophy id: {err}");
        assert!(err.contains("name"), "error should carry the inner serde cause: {err}");
    }

    #[test]
    fn guild_parse_errors_name_the_guild_key() {
        let json = r#"{"111": -1, "222": {"trophies": {"9": {"value": 1}}}}"#;
        let err = parse_guilds(json).expect_err("guild 222 has a broken trophy");
        let msg = format!("{err:#}");
        assert!(msg.contains("guild `222`"), "error should name the guild key: {msg}");
        assert!(msg.contains("trophy `9`"), "error should name the trophy id: {msg}");
    }

    #[test]
    fn legacy_url_enforces_read_only_mode() {
        assert_eq!(legacy_url("./json.sqlite"), "sqlite://./json.sqlite?mode=ro");
        assert_eq!(legacy_url("sqlite://db.sqlite"), "sqlite://db.sqlite?mode=ro");
        assert_eq!(legacy_url("sqlite://db.sqlite?foo=1"), "sqlite://db.sqlite?foo=1&mode=ro");
        // An explicit caller-provided mode is respected.
        assert_eq!(legacy_url("sqlite://db.sqlite?mode=rwc"), "sqlite://db.sqlite?mode=rwc");
    }

    #[test]
    fn legacy_connection_disables_sqlx_statement_logging() {
        // sea-orm defaults sqlx statement logging to INFO, which would interleave
        // raw quick.db SQL with the import report even with DEBUG=false.
        let options = ConnectOptions::new(legacy_url("./json.sqlite")).sqlx_logging(false).to_owned();
        assert!(!options.get_sqlx_logging(), "sqlx statement logging must be off");
        assert_eq!(options.get_url(), "sqlite://./json.sqlite?mode=ro");
    }

    #[tokio::test]
    #[ignore = "requires the git-excluded production snapshot (json.sqlite)"]
    async fn loads_from_sqlite_with_matching_counts() {
        let path = repo_file("json.sqlite");
        let data = LegacyData::load(path.to_str().expect("utf-8 path"))
            .await
            .expect("load json.sqlite");

        assert_eq!(data.guilds.len(), 2_493, "root guild keys");
        assert_eq!(data.tombstone_count(), 5);
        assert_eq!(data.corrupt_count(), 0, "corrupt guild entries (0 expected in production)");
        assert_eq!(data.guilds().count(), 2_488, "valid guilds");

        let trophies: usize = data.guilds().map(|(_, g)| g.trophy_defs().count()).sum();
        assert_eq!(trophies, 10_853);

        assert_eq!(data.bot.commands.get("total"), Some(&104_913));
        assert_eq!(data.bot_stats().get("trophiesAwarded"), Some(&120_411));
    }
}
