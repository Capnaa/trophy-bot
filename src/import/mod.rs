//! Legacy data importer (`trophy-bot import`): quick.db JSON blobs → normalized
//! schema. Algorithm and expected counts: `docs/specs/migration-import.md`.
//!
//! Phases: 0 load/validate (done by `crate::legacy`), 1 bot stats, 2 guilds,
//! 3 trophies, 4 awards, 5 rewards/panels/settings, 6 images, 7 validation +
//! report. Phases 0–6 are prepared in memory (images resolved on disk/network
//! first), then EVERY insert runs in one transaction — all-or-nothing.

mod images;
mod report;

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
    // Same shape for the three optional strings: use the legacy value, or fall
    // back to the column default while reporting the applied default.
    let mut str_or_default = |opt: &Option<String>, field: &'static str, default: &str| -> String {
        match opt {
            Some(value) => value.clone(),
            None => {
                note_default(report, guild_id, legacy_id, field);
                default.to_owned()
            }
        }
    };
    let details = str_or_default(&t.details, "details", DEFAULT_DETAILS);
    let description = str_or_default(&t.description, "description", DEFAULT_DESCRIPTION);
    let emoji = str_or_default(&t.emoji, "emoji", DEFAULT_EMOJI);

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

#[cfg(test)]
mod tests {
    //! Importer tests against a small synthetic legacy fixture (parsed with the
    //! real `crate::legacy` serde model) and an in-memory SQLite target.
    //! Phase rules under test: `docs/specs/migration-import.md`.

    use super::report::MismatchKind;
    use super::{import_data, ImportOptions, DEFAULT_DETAILS};
    use crate::entities::{
        bot_stats, guild_settings, guilds, leaderboard_panels, role_rewards, trophies, user_trophies,
    };
    use crate::legacy::{LegacyBot, LegacyData};
    use crate::migrations::Migrator;
    use sea_orm::{
        ActiveModelTrait, ColumnTrait, ConnectOptions, Database, DatabaseConnection, EntityTrait,
        NotSet, PaginatorTrait, QueryFilter, Set,
    };
    use sea_orm_migration::MigratorTrait;
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::time::Duration;

    /// Synthetic legacy `guilds` document exercising every phase rule:
    /// - guild `100`: renames (two "test"), float rounding (8.5, -2.5), orphan
    ///   award ("99"), duplicate award elements, reward dedupe, panel, partial
    ///   settings, dedications (user+name on `3`, text-only on `4`, absent on
    ///   `1`), incomplete trophy (`2`), score drift (user 502) and
    ///   rounding-induced mismatch (user 500).
    /// - guild `300`: `imsafe` absent, empty settings (no row expected).
    /// - `200`: `/forgetme` tombstone; `999`: corrupt non-object value.
    const FIXTURE: &str = r#"{
      "100": {
        "imsafe": true,
        "settings": {"hide_quit_users": 1, "leaderboard_format": 3},
        "trophies": {
          "current": 5,
          "1": {"name": "test", "value": 10, "creator": "55", "created": 1600000000000,
                "signed": true, "details": "the details", "description": "a desc", "emoji": "🥇"},
          "2": {"name": "test", "value": 8.5},
          "3": {"name": "Unique", "value": -2.5, "creator": "55", "created": 1600000000000,
                "signed": false, "details": "d", "dedication": {"user": "42", "name": "someone"}},
          "4": {"name": "Texty", "value": 1, "creator": "55", "created": 1600000000000,
                "signed": false, "details": "d", "description": "d", "emoji": "🏆",
                "dedication": {"user": null, "name": "For the fans"}}
        },
        "users": {
          "500": {"trophies": ["1", "1", "2", "99"], "trophyValue": 28.5},
          "501": {"trophies": [], "trophyValue": 0},
          "502": {"trophies": ["1"], "trophyValue": 123}
        },
        "rewards": [
          {"role": "700", "requirement": 100},
          {"role": "700", "requirement": 50},
          {"role": "701", "requirement": 5}
        ],
        "panel": {"message": "900", "channel": "901"}
      },
      "300": {
        "settings": {},
        "trophies": {"current": 0},
        "users": {}
      },
      "200": -1,
      "999": "broken"
    }"#;

    async fn fresh_db() -> DatabaseConnection {
        // Single connection: each pooled connection to `sqlite::memory:` would
        // otherwise get its own private database.
        let mut options = ConnectOptions::new("sqlite::memory:");
        options.max_connections(1).sqlx_logging(false);
        let db = Database::connect(options).await.expect("connect to in-memory sqlite");
        Migrator::fresh(&db).await.expect("apply migrations");
        db
    }

    fn legacy_from_json(guilds_json: &str) -> LegacyData {
        LegacyData {
            bot: LegacyBot::default(),
            guilds: serde_json::from_str(guilds_json).expect("parse fixture"),
        }
    }

    /// Options pointing at a directory that does not exist: every local image
    /// reference is "missing" and no URL download can succeed.
    fn opts_no_images() -> ImportOptions {
        ImportOptions {
            images_dir: PathBuf::from("./nonexistent-images-dir-for-tests"),
            http_timeout: Duration::from_secs(1),
            ..Default::default()
        }
    }

    async fn import_fixture() -> (DatabaseConnection, super::ImportReport) {
        let db = fresh_db().await;
        let data = legacy_from_json(FIXTURE);
        let report = import_data(&db, &data, &opts_no_images()).await.expect("import fixture");
        (db, report)
    }

    async fn trophy(db: &DatabaseConnection, guild: i64, legacy_id: &str) -> trophies::Model {
        trophies::Entity::find()
            .filter(trophies::Column::GuildId.eq(guild))
            .filter(trophies::Column::LegacyId.eq(legacy_id))
            .one(db)
            .await
            .expect("query trophy")
            .unwrap_or_else(|| panic!("trophy {guild}/{legacy_id} exists"))
    }

    #[tokio::test]
    async fn refuses_to_import_into_non_empty_target() {
        let db = fresh_db().await;
        let data = legacy_from_json(FIXTURE);
        import_data(&db, &data, &opts_no_images()).await.expect("first import into empty target");

        let err = import_data(&db, &data, &opts_no_images())
            .await
            .expect_err("second import must refuse");
        assert!(
            err.to_string().contains("refusing to import"),
            "error must clearly refuse: {err:#}"
        );

        // Nothing was duplicated by the refused run.
        let count = guilds::Entity::find().count(&db).await.expect("count guilds");
        assert_eq!(count, 2);
    }

    #[tokio::test]
    async fn tombstones_and_corrupt_entries_are_skipped_and_reported() {
        let (db, report) = import_fixture().await;

        assert_eq!(report.tombstoned_guilds, vec!["200".to_string()]);
        // The corrupt entry carries its verbatim legacy value so the pre-cutover
        // review can inspect it without excavating the multi-megabyte blob.
        assert_eq!(report.corrupt_guilds.len(), 1);
        assert_eq!(report.corrupt_guilds[0].key, "999");
        assert_eq!(report.corrupt_guilds[0].value, serde_json::json!("broken"));
        assert_eq!(report.guilds, 2);

        let ids: Vec<i64> = guilds::Entity::find()
            .all(&db)
            .await
            .expect("query guilds")
            .into_iter()
            .map(|g| g.id)
            .collect();
        assert_eq!(ids.len(), 2);
        assert!(ids.contains(&100) && ids.contains(&300), "only valid guilds imported: {ids:?}");
    }

    #[tokio::test]
    async fn imsafe_absent_imports_as_false() {
        let (db, _) = import_fixture().await;

        let safe = guilds::Entity::find_by_id(100).one(&db).await.unwrap().expect("guild 100");
        assert!(safe.is_safe);
        let unsafe_guild = guilds::Entity::find_by_id(300).one(&db).await.unwrap().expect("guild 300");
        assert!(!unsafe_guild.is_safe, "absent imsafe must import as false");
    }

    #[tokio::test]
    async fn float_values_rounded_half_away_from_zero_and_reported() {
        let (db, report) = import_fixture().await;

        assert_eq!(trophy(&db, 100, "2").await.value, 9, "8.5 rounds away from zero to 9");
        assert_eq!(trophy(&db, 100, "3").await.value, -3, "-2.5 rounds away from zero to -3");
        assert_eq!(trophy(&db, 100, "1").await.value, 10, "integer values unchanged");

        assert_eq!(report.rounded_values.len(), 2);
        let by_id: HashMap<&str, (f64, i32)> = report
            .rounded_values
            .iter()
            .map(|r| (r.legacy_id.as_str(), (r.original, r.rounded)))
            .collect();
        assert_eq!(by_id.get("2"), Some(&(8.5, 9)));
        assert_eq!(by_id.get("3"), Some(&(-2.5, -3)));
    }

    #[tokio::test]
    async fn duplicate_names_renamed_per_plan_and_reported() {
        let (db, report) = import_fixture().await;

        assert_eq!(trophy(&db, 100, "1").await.name, "test 1");
        assert_eq!(trophy(&db, 100, "2").await.name, "test 2");
        assert_eq!(trophy(&db, 100, "3").await.name, "Unique", "non-colliding names untouched");

        assert_eq!(report.renamed_trophies.len(), 2);
        for rename in &report.renamed_trophies {
            assert_eq!(rename.guild_id, 100);
            assert_eq!(rename.old_name, "test");
        }
        // The UNIQUE(guild_id, normalized_name) constraint held, so the stored
        // normalized names must be distinct.
        assert_ne!(
            trophy(&db, 100, "1").await.normalized_name,
            trophy(&db, 100, "2").await.normalized_name
        );
    }

    #[tokio::test]
    async fn awards_one_row_per_element_with_null_awarded_by() {
        let (db, report) = import_fixture().await;

        // 500 → "1","1","2" (orphan "99" dropped); 502 → "1".
        assert_eq!(report.awards_inserted, 4);
        let all = user_trophies::Entity::find().all(&db).await.expect("query awards");
        assert_eq!(all.len(), 4);
        assert!(all.iter().all(|a| a.awarded_by.is_none()), "legacy never tracked awarded_by");

        let trophy_one = trophy(&db, 100, "1").await;
        let user_500: Vec<_> = all.iter().filter(|a| a.user_id == 500).collect();
        assert_eq!(user_500.len(), 3, "duplicates are one row each");
        assert_eq!(
            user_500.iter().filter(|a| a.trophy_id == trophy_one.id).count(),
            2,
            "the duplicated element produces two rows for the same trophy"
        );
        // Distinct UUIDv7 primary keys per row.
        let mut ids: Vec<_> = all.iter().map(|a| a.id).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), 4);
    }

    #[tokio::test]
    async fn orphan_award_elements_are_dropped_and_reported() {
        let (db, report) = import_fixture().await;

        assert_eq!(report.orphaned_awards.len(), 1);
        let orphan = &report.orphaned_awards[0];
        assert_eq!((orphan.guild_id, orphan.user_id, orphan.legacy_trophy_id.as_str()), (100, 500, "99"));

        // No award row points at a nonexistent trophy.
        let trophy_ids: Vec<_> = trophies::Entity::find()
            .all(&db)
            .await
            .expect("query trophies")
            .into_iter()
            .map(|t| t.id)
            .collect();
        for award in user_trophies::Entity::find().all(&db).await.expect("query awards") {
            assert!(trophy_ids.contains(&award.trophy_id));
        }
    }

    #[tokio::test]
    async fn empty_award_arrays_produce_no_rows() {
        let (db, report) = import_fixture().await;

        let rows = user_trophies::Entity::find()
            .filter(user_trophies::Column::UserId.eq(501))
            .all(&db)
            .await
            .expect("query user 501 awards");
        assert!(rows.is_empty());
        assert_eq!(report.empty_award_users, 1);
        assert_eq!(report.users_with_awards, 2);
    }

    #[tokio::test]
    async fn rewards_deduped_keeping_lowest_requirement() {
        let (db, report) = import_fixture().await;

        let rows = role_rewards::Entity::find().all(&db).await.expect("query rewards");
        let by_role: HashMap<i64, i32> = rows.iter().map(|r| (r.role_id, r.requirement)).collect();
        assert_eq!(rows.len(), 2);
        assert_eq!(by_role.get(&700), Some(&50), "lowest requirement kept for the duplicated role");
        assert_eq!(by_role.get(&701), Some(&5));

        assert_eq!(report.role_rewards, 2);
        assert_eq!(report.deduped_rewards.len(), 1);
        let removed = &report.deduped_rewards[0];
        assert_eq!(
            (removed.guild_id, removed.role_id, removed.kept_requirement, removed.removed_requirement),
            (100, 700, 50, 100)
        );
    }

    #[tokio::test]
    async fn settings_rows_only_for_present_keys() {
        let (db, report) = import_fixture().await;

        assert_eq!(report.settings_rows, 1);
        let row = guild_settings::Entity::find_by_id(100)
            .one(&db)
            .await
            .expect("query settings")
            .expect("guild 100 has a settings row");
        assert_eq!(row.hide_quit_users, Some(1));
        assert_eq!(row.leaderboard_format, Some(3));
        assert_eq!(row.dedication_display, None, "absent keys stay NULL");
        assert_eq!(row.stack_roles, None);
        assert_eq!(row.hide_unused_trophies, None);

        let none = guild_settings::Entity::find_by_id(300).one(&db).await.expect("query 300");
        assert!(none.is_none(), "empty legacy settings map produces no row");
    }

    #[tokio::test]
    async fn panels_imported_as_is() {
        let (db, report) = import_fixture().await;

        assert_eq!(report.panels, 1);
        let panel = leaderboard_panels::Entity::find_by_id(100)
            .one(&db)
            .await
            .expect("query panel")
            .expect("guild 100 panel");
        assert_eq!(panel.channel_id, 901);
        assert_eq!(panel.message_id, 900);
        assert!(leaderboard_panels::Entity::find_by_id(300)
            .one(&db)
            .await
            .expect("query 300 panel")
            .is_none());
    }

    #[tokio::test]
    async fn incomplete_trophies_get_defaults_and_are_reported() {
        let (db, report) = import_fixture().await;

        let incomplete = trophy(&db, 100, "2").await;
        assert_eq!(incomplete.creator_user_id, None);
        assert!(!incomplete.signed);
        assert_eq!(incomplete.details, DEFAULT_DETAILS);

        let defaulted: Vec<&str> = report
            .defaulted_fields
            .iter()
            .filter(|d| d.legacy_id == "2")
            .map(|d| d.field)
            .collect();
        for field in ["creator", "created", "signed", "details"] {
            assert!(defaulted.contains(&field), "field `{field}` must be reported: {defaulted:?}");
        }

        // Complete trophy keeps its legacy data, including the ms timestamp.
        let complete = trophy(&db, 100, "1").await;
        assert_eq!(complete.creator_user_id, Some(55));
        assert!(complete.signed);
        assert_eq!(complete.details, "the details");
        assert_eq!(
            complete.created_at,
            chrono::DateTime::from_timestamp_millis(1_600_000_000_000).unwrap().naive_utc()
        );

        // Dedication normalization: user + name → both columns.
        let dedicated = trophy(&db, 100, "3").await;
        assert_eq!(dedicated.dedication_user_id, Some(42));
        assert_eq!(dedicated.dedication_text.as_deref(), Some("someone"));
        assert_eq!(complete.dedication_user_id, None);
        assert_eq!(complete.dedication_text, None);
    }

    /// Third documented legacy dedication shape (`data-model-legacy.md`, 496 in
    /// production): `{"user": null, "name": "free text"}` → text only, no user.
    #[tokio::test]
    async fn text_only_dedication_sets_text_without_user() {
        let (db, report) = import_fixture().await;

        let texty = trophy(&db, 100, "4").await;
        assert_eq!(texty.dedication_user_id, None, "text-only dedication has no user id");
        assert_eq!(texty.dedication_text.as_deref(), Some("For the fans"));
        // A normal shape, not an anomaly: nothing reported for this trophy.
        assert!(report.defaulted_fields.iter().all(|d| d.legacy_id != "4"));
        assert!(report.invalid_fields.is_empty());
    }

    /// Defense paths (spec principle 3): present-but-unusable `creator`,
    /// `created` and `dedication.user` are NULLed/defaulted AND reported,
    /// exactly like the orphan-award defense. Production has 0 of these.
    #[tokio::test]
    async fn invalid_present_field_values_are_nulled_and_reported() {
        // `created` is i64::MAX ms — rejected by chrono's from_timestamp_millis.
        let fixture = r#"{
          "600": {
            "trophies": {
              "current": 1,
              "1": {"name": "Odd", "value": 1, "creator": "not-a-snowflake",
                    "created": 9223372036854775807, "signed": false, "details": "d",
                    "description": "d", "emoji": "🏆",
                    "dedication": {"user": "someone", "name": "text"}}
            },
            "users": {}
          }
        }"#;
        let db = fresh_db().await;
        let report =
            import_data(&db, &legacy_from_json(fixture), &opts_no_images()).await.expect("import");

        let odd = trophy(&db, 600, "1").await;
        assert_eq!(odd.creator_user_id, None, "non-numeric creator → NULL");
        assert_eq!(odd.dedication_user_id, None, "non-numeric dedication user → NULL");
        assert_eq!(odd.dedication_text.as_deref(), Some("text"), "dedication text still kept");

        let invalid: HashMap<&str, &str> = report
            .invalid_fields
            .iter()
            .map(|f| {
                assert_eq!((f.guild_id, f.legacy_id.as_str()), (600, "1"));
                (f.field, f.value.as_str())
            })
            .collect();
        assert_eq!(invalid.len(), 3);
        assert_eq!(invalid.get("creator"), Some(&"not-a-snowflake"));
        assert_eq!(invalid.get("created"), Some(&"9223372036854775807"));
        assert_eq!(invalid.get("dedication.user"), Some(&"someone"));
        // Invalid values are not double-reported as absent-field defaults.
        assert!(report.defaulted_fields.is_empty());
    }

    /// Defense (spec principle 3): a legacy trophy value beyond the ±999,999
    /// schema CHECK range is clamped to the nearest bound AND reported, instead
    /// of aborting the whole all-or-nothing import with an opaque chunk-level
    /// CHECK violation that names no guild or trophy. Production has 0 of these.
    #[tokio::test]
    async fn out_of_range_trophy_values_clamped_and_reported() {
        let fixture = r#"{
          "600": {
            "trophies": {
              "current": 2,
              "1": {"name": "TooBig", "value": 10000000, "creator": "1", "created": 1,
                    "signed": false, "details": "d"},
              "2": {"name": "TooSmall", "value": -10000000.5, "creator": "1", "created": 1,
                    "signed": false, "details": "d"}
            },
            "users": {"500": {"trophies": ["1"], "trophyValue": 10000000}}
          }
        }"#;
        let db = fresh_db().await;
        let report = import_data(&db, &legacy_from_json(fixture), &opts_no_images())
            .await
            .expect("out-of-range values must not abort the import");

        assert_eq!(trophy(&db, 600, "1").await.value, 999_999, "clamped to the upper CHECK bound");
        assert_eq!(trophy(&db, 600, "2").await.value, -999_999, "clamped to the lower CHECK bound");

        assert_eq!(report.invalid_fields.len(), 2);
        assert!(report.invalid_fields.iter().all(|f| f.guild_id == 600 && f.field == "value"));
        let invalid: HashMap<&str, &str> =
            report.invalid_fields.iter().map(|f| (f.legacy_id.as_str(), f.value.as_str())).collect();
        assert_eq!(invalid.get("1"), Some(&"10000000"));
        assert_eq!(invalid.get("2"), Some(&"-10000000.5"));
        // The clamp is reported as invalid, not double-reported as a rounding.
        assert!(report.rounded_values.is_empty());
        // The award of the clamped trophy still imports.
        assert_eq!(report.awards_inserted, 1);
    }

    /// Defense: a present setting index outside its column's CHECK range imports
    /// as NULL (the code-side default applies, like an absent key) AND is
    /// reported, instead of aborting on the guild_settings CHECK constraint.
    #[tokio::test]
    async fn out_of_range_setting_indexes_nulled_and_reported() {
        let fixture = r#"{
          "600": {
            "settings": {"stack_roles": 5, "leaderboard_format": 2},
            "trophies": {"current": 0},
            "users": {}
          },
          "601": {
            "settings": {"dedication_display": -1},
            "trophies": {"current": 0},
            "users": {}
          }
        }"#;
        let db = fresh_db().await;
        let report = import_data(&db, &legacy_from_json(fixture), &opts_no_images())
            .await
            .expect("out-of-range setting must not abort the import");

        // Guild 600 keeps a row for its valid key; the invalid one stays NULL.
        let row = guild_settings::Entity::find_by_id(600)
            .one(&db)
            .await
            .expect("query settings")
            .expect("guild 600 has a settings row");
        assert_eq!(row.stack_roles, None, "out-of-range index → NULL (code-side default)");
        assert_eq!(row.leaderboard_format, Some(2), "valid keys unaffected");

        // Guild 601's only key is invalid → no row at all, like empty settings.
        assert!(guild_settings::Entity::find_by_id(601).one(&db).await.expect("query 601").is_none());

        assert_eq!(report.invalid_fields.len(), 2);
        assert!(report.invalid_fields.iter().all(|f| f.field == "setting"));
        let invalid: HashMap<(&str, i64), &str> = report
            .invalid_fields
            .iter()
            .map(|f| ((f.legacy_id.as_str(), f.guild_id), f.value.as_str()))
            .collect();
        assert_eq!(invalid.get(&("stack_roles", 600)), Some(&"5"));
        assert_eq!(invalid.get(&("dedication_display", 601)), Some(&"-1"));
    }

    /// Defense: a reward entry whose requirement violates the schema
    /// `CHECK (requirement >= 1)` (or exceeds i32) is dropped AND reported,
    /// instead of aborting on the role_rewards CHECK constraint.
    #[tokio::test]
    async fn invalid_reward_requirements_dropped_and_reported() {
        let fixture = r#"{
          "600": {
            "trophies": {"current": 0},
            "users": {},
            "rewards": [
              {"role": "700", "requirement": 0},
              {"role": "701", "requirement": 5},
              {"role": "702", "requirement": 4294967296}
            ]
          }
        }"#;
        let db = fresh_db().await;
        let report = import_data(&db, &legacy_from_json(fixture), &opts_no_images())
            .await
            .expect("invalid reward requirements must not abort the import");

        let rows = role_rewards::Entity::find().all(&db).await.expect("query rewards");
        assert_eq!(rows.len(), 1, "only the valid reward is imported");
        assert_eq!((rows[0].role_id, rows[0].requirement), (701, 5));
        assert_eq!(report.role_rewards, 1);

        assert_eq!(report.invalid_fields.len(), 2);
        assert!(report.invalid_fields.iter().all(|f| f.guild_id == 600
            && f.field == "reward.requirement"));
        let invalid: HashMap<&str, &str> =
            report.invalid_fields.iter().map(|f| (f.legacy_id.as_str(), f.value.as_str())).collect();
        assert_eq!(invalid.get("700"), Some(&"0"), "below the CHECK minimum of 1");
        assert_eq!(invalid.get("702"), Some(&"4294967296"), "beyond i32");
        assert!(report.deduped_rewards.is_empty(), "dropped entries are not dedupe removals");
    }

    /// The emptiness check must also cover `bot_stats` — the one imported table
    /// not FK-anchored to `guilds` — so a target that ran with zero guilds gets
    /// the clear refusal instead of a mid-transaction UNIQUE(bot_stats.name)
    /// failure.
    #[tokio::test]
    async fn refuses_when_only_bot_stats_rows_exist() {
        let db = fresh_db().await;
        let now = chrono::Utc::now().naive_utc();
        bot_stats::ActiveModel {
            id: NotSet,
            name: Set("total".to_owned()),
            total: Set(1),
            created_at: Set(now),
            updated_at: Set(now),
        }
        .insert(&db)
        .await
        .expect("seed a bot_stats row");

        let err = import_data(&db, &legacy_from_json("{}"), &opts_no_images())
            .await
            .expect_err("must refuse a target with bot_stats rows");
        assert!(err.to_string().contains("refusing to import"), "clear refusal expected: {err:#}");

        let count = bot_stats::Entity::find().count(&db).await.expect("count bot_stats");
        assert_eq!(count, 1, "the refused run must not touch existing rows");
    }

    /// All-or-nothing (spec principle 2): a mid-transaction insert failure must
    /// roll back every insert that already ran. The trigger is two root keys
    /// (`100` and `0100`) parsing to the same guild id → guilds PK violation,
    /// after the bot_stats phase already inserted its rows. (Out-of-range
    /// settings/values/requirements can no longer trigger this: they are caught
    /// and reported during in-memory preparation instead of reaching a CHECK.)
    #[tokio::test]
    async fn failed_insert_rolls_back_all_prior_phases() {
        let fixture = r#"{
          "100": {
            "imsafe": true,
            "trophies": {
              "current": 1,
              "1": {"name": "T", "value": 10, "creator": "55", "created": 1600000000000,
                    "signed": false, "details": "d"}
            },
            "users": {"500": {"trophies": ["1"], "trophyValue": 10}},
            "rewards": [{"role": "700", "requirement": 5}],
            "panel": {"message": "900", "channel": "901"}
          },
          "0100": {"trophies": {"current": 0}, "users": {}}
        }"#;
        let db = fresh_db().await;
        let err = import_data(&db, &legacy_from_json(fixture), &opts_no_images())
            .await
            .expect_err("duplicate guild ids must fail the import");
        assert!(err.to_string().contains("inserting guilds"), "failure is the guilds insert: {err:#}");

        // Every phase that inserted before the failure was rolled back.
        assert_eq!(bot_stats::Entity::find().count(&db).await.unwrap(), 0, "bot_stats rolled back");
        assert_eq!(guilds::Entity::find().count(&db).await.unwrap(), 0, "guilds rolled back");
        assert_eq!(trophies::Entity::find().count(&db).await.unwrap(), 0, "trophies rolled back");
        assert_eq!(user_trophies::Entity::find().count(&db).await.unwrap(), 0, "awards rolled back");
        assert_eq!(role_rewards::Entity::find().count(&db).await.unwrap(), 0, "rewards rolled back");
        assert_eq!(leaderboard_panels::Entity::find().count(&db).await.unwrap(), 0, "panels rolled back");
        assert_eq!(guild_settings::Entity::find().count(&db).await.unwrap(), 0, "settings rolled back");
    }

    #[tokio::test]
    async fn score_mismatches_classified_as_rounding_or_legacy_drift() {
        let (_db, report) = import_fixture().await;

        assert_eq!(report.score_mismatches.len(), 2);
        let by_user: HashMap<i64, &super::report::ScoreMismatch> =
            report.score_mismatches.iter().map(|m| (m.user_id, m)).collect();

        // User 500: stored 28.5 == raw sum (10+10+8.5) but != rounded sum 29.
        let rounding = by_user.get(&500).expect("user 500 mismatch");
        assert_eq!(rounding.kind, MismatchKind::Rounding);
        assert_eq!(rounding.recalculated, 29);
        assert_eq!(rounding.stored, 28.5);

        // User 502: stored 123 vs raw/rounded 10 — genuine legacy drift.
        let drift = by_user.get(&502).expect("user 502 mismatch");
        assert_eq!(drift.kind, MismatchKind::LegacyDrift);
        assert_eq!(drift.recalculated, 10);

        // User 501 (0 == 0) is not a mismatch — validated by len() == 2 above.
    }

    #[tokio::test]
    async fn bot_stats_imported_as_historical_record() {
        let db = fresh_db().await;
        let bot: LegacyBot = serde_json::from_str(
            r#"{"commands":{"total":10,"award":7},"trophies":3,"trophiesAwarded":5}"#,
        )
        .expect("parse bot fixture");
        let data = LegacyData { bot, guilds: serde_json::from_str("{}").expect("empty guilds") };

        let report = import_data(&db, &data, &opts_no_images()).await.expect("import");
        assert_eq!(report.bot_stats_rows, 4);

        let rows: HashMap<String, i64> = bot_stats::Entity::find()
            .all(&db)
            .await
            .expect("query bot stats")
            .into_iter()
            .map(|r| (r.name, r.total))
            .collect();
        assert_eq!(rows.get("total"), Some(&10));
        assert_eq!(rows.get("award"), Some(&7));
        assert_eq!(rows.get("trophiesAwarded"), Some(&5));
        assert_eq!(rows.get("rootTrophies"), Some(&3));
    }

    const IMAGE_FIXTURE: &str = r#"{
      "400": {
        "trophies": {
          "current": 4,
          "1": {"name": "HasFile", "value": 1, "creator": "1", "created": 1, "signed": false,
                "details": "d", "image": "400_1.png"},
          "2": {"name": "MissingFile", "value": 1, "creator": "1", "created": 1, "signed": false,
                "details": "d", "image": "400_2.png"},
          "3": {"name": "Remote", "value": 1, "creator": "1", "created": 1, "signed": false,
                "details": "d", "image": "http://127.0.0.1:9/pic.gif?ex=deadbeef"}
        },
        "users": {}
      }
    }"#;

    fn temp_images_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("trophy-import-test-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create temp images dir");
        dir
    }

    #[tokio::test]
    async fn images_local_kept_missing_nulled_urls_expired_orphans_listed() {
        let dir = temp_images_dir("images");
        std::fs::write(dir.join("400_1.png"), b"png").expect("write referenced file");
        std::fs::write(dir.join("orphan.png"), b"png").expect("write orphan file");

        let db = fresh_db().await;
        let data = legacy_from_json(IMAGE_FIXTURE);
        let opts = ImportOptions {
            images_dir: dir.clone(),
            http_timeout: Duration::from_secs(1),
            ..Default::default()
        };
        let report = import_data(&db, &data, &opts).await.expect("import never fails on images");

        assert_eq!(trophy(&db, 400, "1").await.image.as_deref(), Some("400_1.png"));
        assert_eq!(trophy(&db, 400, "2").await.image, None, "missing file → NULL");
        assert_eq!(trophy(&db, 400, "3").await.image, None, "dead URL → NULL");

        assert_eq!(report.local_images_kept, 1);
        assert_eq!(report.missing_image_files.len(), 1);
        assert_eq!(report.missing_image_files[0].filename, "400_2.png");
        assert_eq!(report.expired_image_urls.len(), 1);
        assert!(report.expired_image_urls[0].url.starts_with("http://127.0.0.1:9/"));
        assert!(report.downloaded_images.is_empty());
        assert_eq!(report.url_images(), 1);
        assert_eq!(report.orphan_disk_files, vec!["orphan.png".to_string()]);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Minimal local HTTP server: answers every connection with `200 OK` + `body`.
    async fn serve_images(listener: tokio::net::TcpListener, body: &'static [u8]) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        while let Ok((mut sock, _)) = listener.accept().await {
            tokio::spawn(async move {
                let mut buf = [0u8; 2048];
                let _ = sock.read(&mut buf).await; // request headers; content ignored
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = sock.write_all(head.as_bytes()).await;
                let _ = sock.write_all(body).await;
                let _ = sock.shutdown().await;
            });
        }
    }

    /// Phase 6 success path: a live CDN URL is downloaded to the images dir as
    /// `{guild}_{legacy_id}.{ext}`, the trophy stores that filename, and the
    /// download is reported.
    #[tokio::test]
    async fn url_images_downloaded_saved_and_reported() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind local server");
        let port = listener.local_addr().expect("local addr").port();
        tokio::spawn(serve_images(listener, b"GIFDATA"));

        let dir = temp_images_dir("download");
        let fixture = format!(
            r#"{{
              "400": {{
                "trophies": {{
                  "current": 1,
                  "1": {{"name": "Remote", "value": 1, "creator": "1", "created": 1, "signed": false,
                        "details": "d", "image": "http://127.0.0.1:{port}/pic.gif?ex=deadbeef"}}
                }},
                "users": {{}}
              }}
            }}"#
        );
        let db = fresh_db().await;
        let opts = ImportOptions {
            images_dir: dir.clone(),
            http_timeout: Duration::from_secs(5),
            ..Default::default()
        };
        let report = import_data(&db, &legacy_from_json(&fixture), &opts).await.expect("import");

        assert_eq!(trophy(&db, 400, "1").await.image.as_deref(), Some("400_1.gif"));
        assert_eq!(
            std::fs::read(dir.join("400_1.gif")).expect("downloaded file exists"),
            b"GIFDATA",
            "downloaded bytes written to the images dir"
        );

        assert_eq!(report.downloaded_images.len(), 1);
        let downloaded = &report.downloaded_images[0];
        assert_eq!(
            (downloaded.guild_id, downloaded.legacy_id.as_str(), downloaded.filename.as_str()),
            (400, "1", "400_1.gif")
        );
        assert!(downloaded.url.starts_with("http://127.0.0.1:"));
        assert!(report.expired_image_urls.is_empty());
        assert_eq!(report.url_images(), 1);
        assert!(
            report.orphan_disk_files.is_empty(),
            "the downloaded file is referenced, not an orphan: {:?}",
            report.orphan_disk_files
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn report_serializes_to_json_with_summary() {
        let (_db, report) = import_fixture().await;

        let json = serde_json::to_value(&report).expect("report serializes");
        for key in [
            "guilds",
            "tombstoned_guilds",
            "corrupt_guilds",
            "trophies",
            "defaulted_fields",
            "invalid_fields",
            "rounded_values",
            "renamed_trophies",
            "awards_inserted",
            "orphaned_awards",
            "deduped_rewards",
            "panels",
            "settings_rows",
            "missing_image_files",
            "expired_image_urls",
            "downloaded_images",
            "orphan_disk_files",
            "score_mismatches",
        ] {
            assert!(json.get(key).is_some(), "report JSON must contain `{key}`");
        }

        // Summary compares measured vs the production-expected counts.
        let rows = report.summary_rows();
        let row = |name: &str| -> (u64, u64) {
            let (_, measured, expected) =
                rows.iter().find(|(n, _, _)| *n == name).unwrap_or_else(|| panic!("`{name}` row"));
            (*measured, *expected)
        };
        assert_eq!(row("guilds"), (2, 2_488));

        // Every spec-stated production count is machine-checked in the summary
        // (migration-import.md), not just present as a raw JSON field:
        // 43 incomplete trophies (Phase 3), 1,284 empty-award users (Phase 4),
        // 162 guilds with non-empty settings (Phase 5), 2,693 − 200 = 2,493 local
        // images kept and 278 orphan disk files (Phase 6).
        assert_eq!(
            row("defaulted_trophies"),
            (1, 43),
            "only fixture trophy 100/2 misses CORE fields (creator/created/signed); 100/3 misses \
             only description+emoji, which never count toward the spec's 43 incomplete trophies"
        );
        assert_eq!(
            row("defaulted_details"),
            (1, 360),
            "fixture trophy 100/2 also misses `details` (expected legacy shape, tracked separately)"
        );
        assert_eq!(row("empty_award_users"), (1, 1_284), "fixture user 501 has an empty array");
        assert_eq!(row("settings_rows"), (1, 162), "guild 100 only; empty settings get no row");
        assert_eq!(row("local_images_kept"), (0, 2_493), "no images dir in this fixture");
        assert_eq!(row("orphan_disk_files"), (0, 278), "no images dir in this fixture");
    }

    /// The spec's expected 43 counts incomplete TROPHIES (distinct per guild,
    /// missing a CORE field: creator/created/signed), while `defaulted_fields`
    /// records one entry per absent FIELD. `details`-only defaults are expected
    /// legacy shape and tracked in their own metric (expected 360).
    #[test]
    fn defaulted_trophies_counts_distinct_core_incomplete_trophies() {
        use super::report::{DefaultedField, ImportReport};

        let mut report = ImportReport::default();
        for (guild_id, legacy_id, field) in [
            (100, "2", "creator"),
            (100, "2", "created"),
            (100, "2", "signed"),
            (100, "7", "details"), // details-only: excluded from the 43-metric
            (300, "2", "creator"), // same legacy id, different guild
        ] {
            report.defaulted_fields.push(DefaultedField {
                guild_id,
                legacy_id: legacy_id.to_string(),
                field,
            });
        }

        assert_eq!(report.defaulted_trophies(), 2, "100/2 and 300/2 miss core fields; 100/7 not");
        assert_eq!(report.defaulted_details(), 1, "only 100/7 misses details");
        let rows = report.summary_rows();
        let (_, measured, expected) =
            rows.iter().find(|(n, _, _)| *n == "defaulted_trophies").expect("defaulted_trophies row");
        assert_eq!((*measured, *expected), (2, 43));
    }
}
