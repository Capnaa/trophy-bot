//! Legacy data importer (`trophy-bot import`): quick.db JSON blobs → normalized
//! schema. Algorithm and expected counts: `docs/specs/migration-import.md`.
//!
//! Phases: 0 load/validate (done by `crate::legacy`), 1 bot stats, 2 guilds,
//! 3 trophies, 4 awards, 5 rewards/panels/settings, 6 images, 7 validation +
//! report. Phases 0–6 are prepared in memory (images resolved on disk/network
//! first), then EVERY insert runs in one transaction — all-or-nothing.

mod images;
mod report;
#[cfg(test)]
mod tests;

pub use report::*;

use crate::domain::normalize::{normalize_name, plan_renames};
use crate::entities::{
    bot_stats, guild_settings, guilds, leaderboard_panels, role_rewards, trophies, user_trophies,
};
use crate::legacy::{GuildEntry, LegacyData, LegacyGuild, LegacyTrophy, LegacyUser};
use anyhow::{bail, Context, Result};
use chrono::NaiveDateTime;
use sea_orm::{
    ActiveModelTrait, ConnectionTrait, DatabaseConnection, EntityTrait, NotSet, PaginatorTrait,
    Set, TransactionTrait,
};
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::time::Duration;
use uuid::Uuid;

/// Where [`run`] writes the JSON import report.
pub const DEFAULT_REPORT_PATH: &str = "./import-report.json";

/// Defaults applied to incomplete legacy trophies (Phase 3, reported).
const DEFAULT_DESCRIPTION: &str = "No description provided";
const DEFAULT_EMOJI: &str = "🏆";
pub(crate) const DEFAULT_DETAILS: &str = "No details provided.";

/// Rows per batched `insert_many` — 500 rows × ≤16 columns stays well under
/// SQLite's bind-variable limit while keeping the 60k award inserts fast.
const INSERT_CHUNK: usize = 500;

/// Float tolerance for Phase 7 score comparisons (spec: |diff| > 0.001).
const SCORE_TOLERANCE: f64 = 0.001;

/// Schema CHECK bounds for `trophies.value` (initial schema migration).
/// Legacy values outside this range (0 in production) are clamped to the
/// nearest bound and reported, instead of aborting the import transaction.
const TROPHY_VALUE_MIN: f64 = -999_999.0;
const TROPHY_VALUE_MAX: f64 = 999_999.0;

/// Tunables for the importer; production uses [`Default`].
pub struct ImportOptions {
    /// Directory holding legacy trophy images (and receiving CDN downloads).
    pub images_dir: PathBuf,
    /// Per-request timeout for best-effort CDN downloads.
    pub http_timeout: Duration,
    /// Maximum concurrent CDN downloads.
    pub download_concurrency: usize,
}

impl Default for ImportOptions {
    fn default() -> Self {
        Self {
            images_dir: PathBuf::from("./images"),
            http_timeout: Duration::from_secs(5),
            download_concurrency: 16,
        }
    }
}

/// CLI entry point: loads the legacy quick.db file, imports it, writes the
/// JSON report to [`DEFAULT_REPORT_PATH`] and logs the summary table.
pub async fn run(db: &DatabaseConnection, legacy_db_path: &str) -> Result<()> {
    let data = LegacyData::load(legacy_db_path).await?;
    let report = import_data(db, &data, &ImportOptions::default()).await?;

    let json = serde_json::to_string_pretty(&report).context("serializing import report")?;
    std::fs::write(DEFAULT_REPORT_PATH, json)
        .with_context(|| format!("writing {DEFAULT_REPORT_PATH}"))?;
    log::info!("import report written to {DEFAULT_REPORT_PATH}");
    report.log_summary();
    Ok(())
}

/// Imports already-parsed legacy data into the target database.
///
/// Refuses to touch a non-empty target (any `guilds` row). All inserts run in
/// a single transaction; image resolution (disk checks + best-effort CDN
/// downloads) happens before the transaction opens.
pub async fn import_data(
    db: &DatabaseConnection,
    data: &LegacyData,
    opts: &ImportOptions,
) -> Result<ImportReport> {
    ensure_empty_target(db).await?;

    let now = chrono::Utc::now().naive_utc();
    let mut report = ImportReport::default();

    // Phases 0 + 2–5: pure in-memory preparation (tombstones skipped,
    // defaults, rounding, renames, uuid mapping, award resolution, dedupe).
    let mut prepared = prepare(data, now, &mut report)?;

    // Phase 6: images — resolved before the transaction so no network or
    // filesystem I/O happens inside it. Never fails the import.
    images::resolve(&mut prepared, opts, &mut report).await;

    // Phases 1–5: every insert in ONE transaction (all-or-nothing).
    let txn = db.begin().await.context("opening import transaction")?;
    insert_bot_stats(&txn, data, now, &mut report).await?;
    insert_guilds(&txn, &prepared, now, &mut report).await?;
    insert_trophies(&txn, &prepared, now, &mut report).await?;
    insert_awards(&txn, &prepared, now, &mut report).await?;
    insert_rewards(&txn, &prepared, now, &mut report).await?;
    insert_panels(&txn, &prepared, now, &mut report).await?;
    insert_settings(&txn, &prepared, now, &mut report).await?;
    txn.commit().await.context("committing import transaction")?;

    // Phase 7: score validation — report only, never reconciled (ADR 0006).
    validate_scores(&prepared, &mut report);

    log::info!(
        "import finished: {} guilds, {} trophies, {} awards",
        report.guilds,
        report.trophies,
        report.awards_inserted
    );
    Ok(report)
}

/// Idempotent-by-rerun (spec principle 4): never merge into existing data.
///
/// Checks `guilds` AND `bot_stats`: every other imported table hangs off
/// `guilds` via FKs, but `bot_stats` is independent — a target that ran the
/// bot with zero guilds would otherwise pass here and then die mid-transaction
/// on `UNIQUE(bot_stats.name)` with a confusing DB error.
async fn ensure_empty_target(db: &DatabaseConnection) -> Result<()> {
    let guild_rows = guilds::Entity::find()
        .count(db)
        .await
        .context("checking that the target `guilds` table is empty")?;
    let bot_stat_rows = bot_stats::Entity::find()
        .count(db)
        .await
        .context("checking that the target `bot_stats` table is empty")?;
    if guild_rows > 0 || bot_stat_rows > 0 {
        bail!(
            "target database already contains data ({guild_rows} guild row(s), \
             {bot_stat_rows} bot_stats row(s)); refusing to import into a \
             non-empty target (run `trophy-bot fresh` first)"
        );
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// In-memory preparation (Phases 0, 2–5)
// ---------------------------------------------------------------------------

/// Everything needed to insert one guild, resolved and validated in memory.
struct PreparedGuild {
    id: i64,
    is_safe: bool,
    trophies: Vec<PreparedTrophy>,
    awards: Vec<PreparedAward>,
    rewards: Vec<PreparedReward>,
    panel: Option<PreparedPanel>,
    settings: Option<PreparedSettings>,
    score_checks: Vec<ScoreCheck>,
}

struct PreparedTrophy {
    id: Uuid,
    guild_id: i64,
    legacy_id: String,
    creator_user_id: Option<i64>,
    name: String,
    normalized_name: String,
    description: String,
    emoji: String,
    /// Final INTEGER value (legacy floats rounded half-away-from-zero).
    value: i32,
    /// Original legacy value, kept for Phase 7 mismatch classification.
    raw_value: f64,
    /// Legacy image reference; consumed by the images phase.
    image_source: Option<ImageSource>,
    /// Final stored filename, filled by the images phase.
    image: Option<String>,
    dedication_user_id: Option<i64>,
    dedication_text: Option<String>,
    details: String,
    signed: bool,
    created_at: NaiveDateTime,
}

enum ImageSource {
    Local(String),
    Url(String),
}

struct PreparedAward {
    user_id: i64,
    trophy_id: Uuid,
}

struct PreparedReward {
    role_id: i64,
    requirement: i32,
}

struct PreparedPanel {
    channel_id: i64,
    message_id: i64,
}

struct PreparedSettings {
    dedication_display: Option<i16>,
    stack_roles: Option<i16>,
    hide_unused_trophies: Option<i16>,
    hide_quit_users: Option<i16>,
    leaderboard_format: Option<i16>,
}

/// Per-user Phase 7 input: legacy stored score vs both recalculated sums.
struct ScoreCheck {
    user_id: i64,
    stored: f64,
    rounded_sum: i64,
    raw_sum: f64,
}

/// Walks the root guild map in deterministic (numeric) order, skipping and
/// reporting tombstones/corrupt entries, and prepares every valid guild.
fn prepare(
    data: &LegacyData,
    now: NaiveDateTime,
    report: &mut ImportReport,
) -> Result<Vec<PreparedGuild>> {
    let mut keys: Vec<&String> = data.guilds.keys().collect();
    keys.sort_by_key(|k| (k.parse::<u64>().ok(), (*k).clone()));

    let mut prepared = Vec::new();
    for key in keys {
        match &data.guilds[key] {
            GuildEntry::Tombstone => report.tombstoned_guilds.push(key.clone()),
            GuildEntry::Corrupt(value) => report.corrupt_guilds.push(CorruptGuild {
                key: key.clone(),
                value: value.clone(),
            }),
            GuildEntry::Guild(guild) => prepared.push(prepare_guild(key, guild, now, report)?),
        }
    }
    Ok(prepared)
}

fn prepare_guild(
    key: &str,
    guild: &LegacyGuild,
    now: NaiveDateTime,
    report: &mut ImportReport,
) -> Result<PreparedGuild> {
    let guild_id: i64 = key.parse().with_context(|| format!("non-numeric guild key `{key}`"))?;

    let trophies = prepare_trophies(guild_id, guild, now, report);
    let (awards, score_checks) = prepare_awards(guild_id, guild, &trophies, report)?;
    let rewards = prepare_rewards(guild_id, guild, report)?;
    let panel = guild
        .panel
        .as_ref()
        .map(|p| -> Result<PreparedPanel> {
            Ok(PreparedPanel {
                channel_id: p.channel.parse().with_context(|| {
                    format!("guild {guild_id}: non-numeric panel channel `{}`", p.channel)
                })?,
                message_id: p.message.parse().with_context(|| {
                    format!("guild {guild_id}: non-numeric panel message `{}`", p.message)
                })?,
            })
        })
        .transpose()?;
    let settings = prepare_settings(guild_id, guild, report);

    Ok(PreparedGuild {
        id: guild_id,
        // Absent in legacy → false (81 guilds); always `true` when present.
        is_safe: guild.imsafe.unwrap_or(false),
        trophies,
        awards,
        rewards,
        panel,
        settings,
        score_checks,
    })
}

/// Phase 3: defaults, rounding, ADR 0005 renames and UUIDv7 generation for
/// one guild's trophies, in deterministic (numeric legacy-id) order.
fn prepare_trophies(
    guild_id: i64,
    guild: &LegacyGuild,
    now: NaiveDateTime,
    report: &mut ImportReport,
) -> Vec<PreparedTrophy> {
    let mut defs: Vec<(&str, &LegacyTrophy)> = guild.trophy_defs().collect();
    defs.sort_by_key(|(id, _)| (id.parse::<u64>().ok(), id.to_string()));

    let pairs: Vec<(String, String)> =
        defs.iter().map(|(id, t)| ((*id).to_string(), t.name.clone())).collect();
    let mut new_names: HashMap<String, String> = HashMap::new();
    for rename in plan_renames(&pairs) {
        report.renamed_trophies.push(RenamedTrophy {
            guild_id,
            legacy_id: rename.legacy_id.clone(),
            old_name: rename.old_name,
            new_name: rename.new_name.clone(),
        });
        new_names.insert(rename.legacy_id, rename.new_name);
    }

    defs.into_iter()
        .map(|(legacy_id, t)| {
            prepare_trophy(guild_id, legacy_id, t, new_names.remove(legacy_id), now, report)
        })
        .collect()
}

fn note_default(report: &mut ImportReport, guild_id: i64, legacy_id: &str, field: &'static str) {
    report.defaulted_fields.push(DefaultedField {
        guild_id,
        legacy_id: legacy_id.to_owned(),
        field,
    });
}

/// Records a field that was PRESENT in legacy but unusable (spec principle 3:
/// never silently fix). Defense only — production has 0 such values.
fn note_invalid(
    report: &mut ImportReport,
    guild_id: i64,
    legacy_id: &str,
    field: &'static str,
    value: impl Into<String>,
) {
    report.invalid_fields.push(InvalidFieldValue {
        guild_id,
        legacy_id: legacy_id.to_owned(),
        field,
        value: value.into(),
    });
}

fn prepare_trophy(
    guild_id: i64,
    legacy_id: &str,
    t: &LegacyTrophy,
    renamed: Option<String>,
    now: NaiveDateTime,
    report: &mut ImportReport,
) -> PreparedTrophy {
    let creator_user_id = match t.creator.as_deref() {
        // Non-numeric snowflake → NULL, but reported (defense; 0 in prod).
        Some(creator) => match creator.parse::<i64>() {
            Ok(id) => Some(id),
            Err(_) => {
                note_invalid(report, guild_id, legacy_id, "creator", creator);
                None
            }
        },
        None => {
            note_default(report, guild_id, legacy_id, "creator");
            None
        }
    };
    // Legacy `created` is Unix MILLISECONDS; missing → synthetic import time.
    let created_at = match t.created {
        Some(ms) => match chrono::DateTime::from_timestamp_millis(ms) {
            Some(dt) => dt.naive_utc(),
            // Out of chrono's range → synthetic import time, reported.
            None => {
                note_invalid(report, guild_id, legacy_id, "created", ms.to_string());
                now
            }
        },
        None => {
            note_default(report, guild_id, legacy_id, "created");
            now
        }
    };
    let signed = match t.signed {
        Some(signed) => signed,
        None => {
            note_default(report, guild_id, legacy_id, "signed");
            false
        }
    };
    let details = match &t.details {
        Some(details) => details.clone(),
        None => {
            note_default(report, guild_id, legacy_id, "details");
            DEFAULT_DETAILS.to_owned()
        }
    };
    let description = match &t.description {
        Some(description) => description.clone(),
        None => {
            note_default(report, guild_id, legacy_id, "description");
            DEFAULT_DESCRIPTION.to_owned()
        }
    };
    let emoji = match &t.emoji {
        Some(emoji) => emoji.clone(),
        None => {
            note_default(report, guild_id, legacy_id, "emoji");
            DEFAULT_EMOJI.to_owned()
        }
    };

    // f64::round rounds half-way cases away from zero — exactly the spec rule.
    // A rounded value outside the schema CHECK range (±999,999; defense, 0 in
    // production) is clamped to the nearest bound and reported, instead of
    // aborting the whole transaction with an opaque chunk-level CHECK error.
    let raw_value = t.value;
    let rounded = raw_value.round();
    let value = if (TROPHY_VALUE_MIN..=TROPHY_VALUE_MAX).contains(&rounded) {
        if raw_value.fract() != 0.0 {
            report.rounded_values.push(RoundedValue {
                guild_id,
                legacy_id: legacy_id.to_owned(),
                original: raw_value,
                rounded: rounded as i32,
            });
        }
        rounded as i32
    } else {
        note_invalid(report, guild_id, legacy_id, "value", raw_value.to_string());
        if rounded < TROPHY_VALUE_MIN {
            TROPHY_VALUE_MIN as i32
        } else {
            TROPHY_VALUE_MAX as i32
        }
    };

    let name = renamed.unwrap_or_else(|| t.name.clone());
    let normalized_name = normalize_name(&name);

    // Dedication: empty/null shapes → NULLs; text-only → text; user → both.
    // A present, non-empty, non-numeric user is NULLed but reported (defense).
    let dedication_user_id = match t.dedication.user.as_deref().filter(|s| !s.is_empty()) {
        Some(user) => match user.parse::<i64>() {
            Ok(id) => Some(id),
            Err(_) => {
                note_invalid(report, guild_id, legacy_id, "dedication.user", user);
                None
            }
        },
        None => None,
    };
    let dedication_text = t.dedication.name.clone().filter(|s| !s.is_empty());

    let image_source = t.image.as_deref().filter(|s| !s.is_empty()).map(|s| {
        if s.starts_with("http://") || s.starts_with("https://") {
            ImageSource::Url(s.to_owned())
        } else {
            ImageSource::Local(s.to_owned())
        }
    });

    PreparedTrophy {
        id: Uuid::now_v7(),
        guild_id,
        legacy_id: legacy_id.to_owned(),
        creator_user_id,
        name,
        normalized_name,
        description,
        emoji,
        value,
        raw_value,
        image_source,
        image: None,
        dedication_user_id,
        dedication_text,
        details,
        signed,
        created_at,
    }
}

/// Phase 4: one award per array element via the `(guild, legacy_id) → uuid`
/// mapping; mapping misses are dropped and reported. Also accumulates the
/// per-user sums for Phase 7.
fn prepare_awards(
    guild_id: i64,
    guild: &LegacyGuild,
    trophies: &[PreparedTrophy],
    report: &mut ImportReport,
) -> Result<(Vec<PreparedAward>, Vec<ScoreCheck>)> {
    let lookup: HashMap<&str, &PreparedTrophy> =
        trophies.iter().map(|t| (t.legacy_id.as_str(), t)).collect();

    let mut users: Vec<(&String, &LegacyUser)> = guild.users.iter().collect();
    users.sort_by_key(|(id, _)| (id.parse::<u64>().ok(), (*id).clone()));

    let mut awards = Vec::new();
    let mut score_checks = Vec::new();
    for (user_key, user) in users {
        let user_id: i64 = user_key
            .parse()
            .with_context(|| format!("guild {guild_id}: non-numeric user key `{user_key}`"))?;
        if user.trophies.is_empty() {
            report.empty_award_users += 1;
        } else {
            report.users_with_awards += 1;
        }

        let mut rounded_sum: i64 = 0;
        let mut raw_sum: f64 = 0.0;
        for element in &user.trophies {
            match lookup.get(element.as_str()) {
                Some(trophy) => {
                    rounded_sum += i64::from(trophy.value);
                    raw_sum += trophy.raw_value;
                    awards.push(PreparedAward { user_id, trophy_id: trophy.id });
                }
                None => report.orphaned_awards.push(OrphanedAward {
                    guild_id,
                    user_id,
                    legacy_trophy_id: element.clone(),
                }),
            }
        }
        score_checks.push(ScoreCheck {
            user_id,
            stored: user.trophy_value,
            rounded_sum,
            raw_sum,
        });
    }
    Ok((awards, score_checks))
}

/// Phase 5: dedupe duplicate role IDs keeping the LOWEST requirement (the
/// user-favorable fix for the legacy suppression bug), reporting removals.
/// An entry whose requirement violates the schema `CHECK (requirement >= 1)`
/// or exceeds i32 (defense, 0 in production) is dropped and reported instead
/// of aborting the import transaction on the CHECK constraint.
fn prepare_rewards(
    guild_id: i64,
    guild: &LegacyGuild,
    report: &mut ImportReport,
) -> Result<Vec<PreparedReward>> {
    let mut grouped: BTreeMap<i64, Vec<i32>> = BTreeMap::new();
    for entry in &guild.rewards {
        let role_id: i64 = entry.role.parse().with_context(|| {
            format!("guild {guild_id}: non-numeric reward role `{}`", entry.role)
        })?;
        let requirement = match i32::try_from(entry.requirement) {
            Ok(requirement) if requirement >= 1 => requirement,
            _ => {
                note_invalid(
                    report,
                    guild_id,
                    &entry.role,
                    "reward.requirement",
                    entry.requirement.to_string(),
                );
                continue;
            }
        };
        grouped.entry(role_id).or_default().push(requirement);
    }

    let mut rewards = Vec::new();
    for (role_id, mut requirements) in grouped {
        requirements.sort_unstable();
        let kept = requirements[0];
        for removed in &requirements[1..] {
            report.deduped_rewards.push(DedupedReward {
                guild_id,
                role_id,
                kept_requirement: kept,
                removed_requirement: *removed,
            });
        }
        rewards.push(PreparedReward { role_id, requirement: kept });
    }
    Ok(rewards)
}

/// Phase 5: a settings row exists only when at least one key was explicitly
/// present in legacy; absent keys stay NULL (code-side defaults, like
/// legacy `getSetting`). A present index outside its column's schema CHECK
/// range (defense, 0 in production) also stays NULL — treated like an absent
/// key and reported, instead of aborting the import transaction on the CHECK
/// constraint.
fn prepare_settings(
    guild_id: i64,
    guild: &LegacyGuild,
    report: &mut ImportReport,
) -> Option<PreparedSettings> {
    // `max` is each column's schema CHECK upper bound (all lower bounds are 0).
    let mut get = |key: &'static str, max: i64| -> Option<i16> {
        let v = *guild.settings.get(key)?;
        if (0..=max).contains(&v) {
            Some(v as i16)
        } else {
            note_invalid(report, guild_id, key, "setting", v.to_string());
            None
        }
    };
    let settings = PreparedSettings {
        dedication_display: get("dedication_display", 2),
        stack_roles: get("stack_roles", 1),
        hide_unused_trophies: get("hide_unused_trophies", 1),
        hide_quit_users: get("hide_quit_users", 1),
        leaderboard_format: get("leaderboard_format", 3),
    };
    let any_present = settings.dedication_display.is_some()
        || settings.stack_roles.is_some()
        || settings.hide_unused_trophies.is_some()
        || settings.hide_quit_users.is_some()
        || settings.leaderboard_format.is_some();
    any_present.then_some(settings)
}

// ---------------------------------------------------------------------------
// Inserts (Phases 1–5, inside the single transaction)
// ---------------------------------------------------------------------------

/// Batched `insert_many` in [`INSERT_CHUNK`]-row chunks.
async fn insert_chunked<A, C>(db: &C, rows: Vec<A>, what: &str) -> Result<()>
where
    A: ActiveModelTrait + Clone + Send,
    C: ConnectionTrait,
{
    for chunk in rows.chunks(INSERT_CHUNK) {
        <A::Entity as EntityTrait>::insert_many(chunk.to_vec())
            .exec(db)
            .await
            .with_context(|| format!("inserting {what}"))?;
    }
    Ok(())
}

/// Phase 1: per-command counters + the two historical global counters.
async fn insert_bot_stats<C: ConnectionTrait>(
    db: &C,
    data: &LegacyData,
    now: NaiveDateTime,
    report: &mut ImportReport,
) -> Result<()> {
    // BTreeMap for deterministic insert order.
    let stats: BTreeMap<String, u64> = data.bot_stats().into_iter().collect();
    report.bot_stats_rows = stats.len() as u64;
    let rows: Vec<bot_stats::ActiveModel> = stats
        .into_iter()
        .map(|(name, total)| bot_stats::ActiveModel {
            id: NotSet,
            name: Set(name),
            total: Set(total as i64),
            created_at: Set(now),
            updated_at: Set(now),
        })
        .collect();
    insert_chunked(db, rows, "bot_stats").await
}

async fn insert_guilds<C: ConnectionTrait>(
    db: &C,
    prepared: &[PreparedGuild],
    now: NaiveDateTime,
    report: &mut ImportReport,
) -> Result<()> {
    let rows: Vec<guilds::ActiveModel> = prepared
        .iter()
        .map(|g| guilds::ActiveModel {
            id: Set(g.id),
            is_safe: Set(g.is_safe),
            created_at: Set(now),
            updated_at: Set(now),
        })
        .collect();
    report.guilds = rows.len() as u64;
    insert_chunked(db, rows, "guilds").await
}

async fn insert_trophies<C: ConnectionTrait>(
    db: &C,
    prepared: &[PreparedGuild],
    now: NaiveDateTime,
    report: &mut ImportReport,
) -> Result<()> {
    let rows: Vec<trophies::ActiveModel> = prepared
        .iter()
        .flat_map(|g| g.trophies.iter())
        .map(|t| trophies::ActiveModel {
            id: Set(t.id),
            guild_id: Set(t.guild_id),
            legacy_id: Set(Some(t.legacy_id.clone())),
            creator_user_id: Set(t.creator_user_id),
            name: Set(t.name.clone()),
            normalized_name: Set(t.normalized_name.clone()),
            description: Set(t.description.clone()),
            emoji: Set(t.emoji.clone()),
            value: Set(t.value),
            image: Set(t.image.clone()),
            dedication_user_id: Set(t.dedication_user_id),
            dedication_text: Set(t.dedication_text.clone()),
            details: Set(t.details.clone()),
            signed: Set(t.signed),
            created_at: Set(t.created_at),
            updated_at: Set(now),
        })
        .collect();
    report.trophies = rows.len() as u64;
    insert_chunked(db, rows, "trophies").await
}

async fn insert_awards<C: ConnectionTrait>(
    db: &C,
    prepared: &[PreparedGuild],
    now: NaiveDateTime,
    report: &mut ImportReport,
) -> Result<()> {
    let rows: Vec<user_trophies::ActiveModel> = prepared
        .iter()
        .flat_map(|g| {
            g.awards.iter().map(move |a| user_trophies::ActiveModel {
                id: Set(Uuid::now_v7()),
                guild_id: Set(g.id),
                user_id: Set(a.user_id),
                trophy_id: Set(a.trophy_id),
                awarded_by: Set(None), // Legacy never tracked the awarder.
                awarded_at: Set(now),  // Synthetic: legacy has no timestamp.
                created_at: Set(now),
                updated_at: Set(now),
            })
        })
        .collect();
    report.awards_inserted = rows.len() as u64;
    insert_chunked(db, rows, "user_trophies").await
}

async fn insert_rewards<C: ConnectionTrait>(
    db: &C,
    prepared: &[PreparedGuild],
    now: NaiveDateTime,
    report: &mut ImportReport,
) -> Result<()> {
    let rows: Vec<role_rewards::ActiveModel> = prepared
        .iter()
        .flat_map(|g| {
            g.rewards.iter().map(move |r| role_rewards::ActiveModel {
                id: Set(Uuid::now_v7()),
                guild_id: Set(g.id),
                role_id: Set(r.role_id),
                requirement: Set(r.requirement),
                created_at: Set(now),
                updated_at: Set(now),
            })
        })
        .collect();
    report.role_rewards = rows.len() as u64;
    insert_chunked(db, rows, "role_rewards").await
}

async fn insert_panels<C: ConnectionTrait>(
    db: &C,
    prepared: &[PreparedGuild],
    now: NaiveDateTime,
    report: &mut ImportReport,
) -> Result<()> {
    let rows: Vec<leaderboard_panels::ActiveModel> = prepared
        .iter()
        .filter_map(|g| {
            g.panel.as_ref().map(|p| leaderboard_panels::ActiveModel {
                guild_id: Set(g.id),
                channel_id: Set(p.channel_id),
                message_id: Set(p.message_id),
                created_at: Set(now),
                updated_at: Set(now),
            })
        })
        .collect();
    report.panels = rows.len() as u64;
    insert_chunked(db, rows, "leaderboard_panels").await
}

async fn insert_settings<C: ConnectionTrait>(
    db: &C,
    prepared: &[PreparedGuild],
    now: NaiveDateTime,
    report: &mut ImportReport,
) -> Result<()> {
    let rows: Vec<guild_settings::ActiveModel> = prepared
        .iter()
        .filter_map(|g| {
            g.settings.as_ref().map(|s| guild_settings::ActiveModel {
                guild_id: Set(g.id),
                dedication_display: Set(s.dedication_display),
                stack_roles: Set(s.stack_roles),
                hide_unused_trophies: Set(s.hide_unused_trophies),
                hide_quit_users: Set(s.hide_quit_users),
                leaderboard_format: Set(s.leaderboard_format),
                created_at: Set(now),
                updated_at: Set(now),
            })
        })
        .collect();
    report.settings_rows = rows.len() as u64;
    insert_chunked(db, rows, "guild_settings").await
}

// ---------------------------------------------------------------------------
// Phase 7 — score validation (report only)
// ---------------------------------------------------------------------------

/// Compares each user's legacy `trophyValue` against the sum of the ROUNDED
/// stored values (float-tolerant). Mismatches that agree with the RAW float
/// sum were induced by rounding; the rest are genuine legacy drift. Never
/// reconciles — the recalculated value is correct by definition (ADR 0006).
fn validate_scores(prepared: &[PreparedGuild], report: &mut ImportReport) {
    for guild in prepared {
        for check in &guild.score_checks {
            if (check.stored - check.rounded_sum as f64).abs() <= SCORE_TOLERANCE {
                continue;
            }
            let kind = if (check.stored - check.raw_sum).abs() <= SCORE_TOLERANCE {
                MismatchKind::Rounding
            } else {
                MismatchKind::LegacyDrift
            };
            report.score_mismatches.push(ScoreMismatch {
                guild_id: guild.id,
                user_id: check.user_id,
                stored: check.stored,
                recalculated: check.rounded_sum,
                raw_recalculated: check.raw_sum,
                kind,
            });
        }
    }
}
