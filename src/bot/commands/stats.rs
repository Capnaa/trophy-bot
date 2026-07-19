//! `/stats` — global bot statistics (batch C14, F34).
//!
//! Spec: docs/specs/commands-utility.md § /stats. The legacy bot showed three
//! quick.db lifetime counters known to be inflated (they counted failed runs
//! and guilds that later left). Here (F34):
//! - guilds / trophies / awards are **live COUNTs** from the normalized DB;
//! - server and user counts from the gateway cache are labeled as cached;
//! - uptime is real process uptime (instant pinned at command registration);
//! - the `bot_stats` run counters are incremented **success-only**, on top of
//!   the imported historical totals.
//!
//! The counters are recorded for EVERY command by the framework's
//! `post_command` hook (see `record_command_run` in `src/bot/mod.rs`), which
//! calls [`record_successful_run`] after a command action returns `Ok`.

use std::sync::LazyLock;
use std::time::{Duration, Instant};

use anyhow::Context as _;
use poise::serenity_prelude as serenity;
use sea_orm::sea_query::{Expr, ExprTrait, OnConflict};
use sea_orm::{
    ColumnTrait, ConnectionTrait, EntityTrait, NotSet, PaginatorTrait, QueryFilter, Set,
    TransactionSession, TransactionTrait,
};

use crate::bot::{Context, Data, Error, util};
use crate::entities::{bot_stats, guilds, trophies, user_trophies};
use crate::i18n::{self, LanguageIdentifier};

/// `bot_stats` row holding lifetime successful command runs. Matches the
/// imported legacy `data.commands.total` counter (historical, inflated by
/// pre-cutover failed runs — kept as-is per the spec).
const TOTAL_COUNTER: &str = "total";

/// Process start instant, pinned when [`stats`] builds the command at
/// framework construction (i.e. at startup).
static STARTED: LazyLock<Instant> = LazyLock::new(Instant::now);

/// Builds the `/stats` command. Hand-written wrapper around the
/// macro-generated command so the process-start instant is captured eagerly
/// at registration time — the foundation exposes no startup hook and `Data`
/// is outside this batch's ownership.
pub fn stats() -> poise::Command<Data, Error> {
    let _ = *STARTED;
    stats_command()
}

/// Look at the bot stats
#[poise::command(slash_command, rename = "stats", user_cooldown = 10)]
async fn stats_command(ctx: Context<'_>) -> Result<(), Error> {
    let locale = util::locale(&ctx);
    let db = &ctx.data().db;

    let snapshot = load_snapshot(db).await?;

    let cache = &ctx.serenity_context().cache;
    let cached_servers = cache.guild_count() as u64;
    let cached_users = cache.user_count() as u64;
    let registered_commands = ctx.framework().options().commands.len() as u64;
    let uptime = format_uptime(&locale, STARTED.elapsed());

    let discord_value = i18n::t_args(
        &locale,
        "stats-discord-value",
        &[
            ("servers", cached_servers.into()),
            ("users", cached_users.into()),
            ("uptime", uptime.into()),
        ],
    );
    let trophies_value = i18n::t_args(
        &locale,
        "stats-trophies-value",
        &[
            ("commands", registered_commands.into()),
            ("runs", snapshot.command_runs.into()),
            ("guilds", snapshot.guilds.into()),
            ("trophies", snapshot.trophies.into()),
            ("awarded", snapshot.awards.into()),
        ],
    );

    let embed = serenity::CreateEmbed::new()
        .title(i18n::t(&locale, "stats-title"))
        .colour(util::COLOR_MAIN)
        .field(i18n::t(&locale, "stats-discord-label"), discord_value, true)
        .field(i18n::t(&locale, "stats-trophies-label"), trophies_value, true);
    util::reply_embed(ctx, embed, false).await?;

    // Success-only counters (F34) are bumped by the framework `post_command`
    // hook once this returns `Ok` — no per-command bookkeeping here.
    Ok(())
}

/// Live statistics read from the normalized database.
#[derive(Debug, PartialEq, Eq)]
pub struct StatsSnapshot {
    /// `COUNT(*)` of registered guilds.
    pub guilds: u64,
    /// `COUNT(*)` of trophy definitions.
    pub trophies: u64,
    /// `COUNT(*)` of individual awards (`user_trophies` rows).
    pub awards: u64,
    /// Lifetime command runs: imported historical total + successful runs
    /// recorded post-cutover. `0` when the counter row does not exist yet.
    pub command_runs: i64,
}

/// Loads the live counts (F34) plus the lifetime run counter.
pub async fn load_snapshot<C: ConnectionTrait>(db: &C) -> anyhow::Result<StatsSnapshot> {
    let guilds = guilds::Entity::find()
        .count(db)
        .await
        .context("counting guilds")?;
    let trophies = trophies::Entity::find()
        .count(db)
        .await
        .context("counting trophies")?;
    let awards = user_trophies::Entity::find()
        .count(db)
        .await
        .context("counting awards")?;
    let command_runs = bot_stats::Entity::find()
        .filter(bot_stats::Column::Name.eq(TOTAL_COUNTER))
        .one(db)
        .await
        .context("reading the `total` run counter")?
        .map_or(0, |row| row.total);

    Ok(StatsSnapshot { guilds, trophies, awards, command_runs })
}

/// Records one **successful** command run: bumps the global `total` counter
/// and the per-command counter, creating either row on first use (imported
/// historical rows are continued, not reset). Both upserts run in a single
/// transaction so the two counters never drift apart; callers already inside
/// a transaction get a nested savepoint.
pub async fn record_successful_run<C: TransactionTrait>(db: &C, command: &str) -> anyhow::Result<()> {
    let txn = db.begin().await.context("beginning bot_stats counter transaction")?;
    increment_counter(&txn, TOTAL_COUNTER).await?;
    increment_counter(&txn, command).await?;
    txn.commit().await.context("committing bot_stats counters")?;
    Ok(())
}

/// Atomic upsert: `INSERT .. ON CONFLICT(name) DO UPDATE SET total = total + 1`.
async fn increment_counter<C: ConnectionTrait>(db: &C, name: &str) -> anyhow::Result<()> {
    let now = chrono::Utc::now().naive_utc();
    bot_stats::Entity::insert(bot_stats::ActiveModel {
        id: NotSet,
        name: Set(name.to_owned()),
        total: Set(1),
        created_at: Set(now),
        updated_at: Set(now),
    })
    .on_conflict(
        OnConflict::column(bot_stats::Column::Name)
            .value(
                bot_stats::Column::Total,
                Expr::col(bot_stats::Column::Total).add(1),
            )
            .value(bot_stats::Column::UpdatedAt, now)
            .to_owned(),
    )
    .exec_without_returning(db)
    .await
    .with_context(|| format!("incrementing bot_stats counter `{name}`"))?;
    Ok(())
}

/// Splits an uptime into `(days, hours, minutes, seconds)`.
pub fn uptime_parts(uptime: Duration) -> (u64, u64, u64, u64) {
    let secs = uptime.as_secs();
    (secs / 86_400, secs % 86_400 / 3_600, secs % 3_600 / 60, secs % 60)
}

/// Formats an uptime with the localized `Xd Xh Xm Xs` pattern.
pub fn format_uptime(locale: &LanguageIdentifier, uptime: Duration) -> String {
    let (days, hours, minutes, seconds) = uptime_parts(uptime);
    i18n::t_args(
        locale,
        "stats-uptime-value",
        &[
            ("days", days.into()),
            ("hours", hours.into()),
            ("minutes", minutes.into()),
            ("seconds", seconds.into()),
        ],
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::test_support::{fresh_db, insert_guild, now};
    use sea_orm::{ActiveModelTrait, DatabaseConnection};
    use uuid::Uuid;

    async fn seed_trophy(db: &DatabaseConnection, guild_id: i64, name: &str) -> Uuid {
        let id = Uuid::now_v7();
        trophies::ActiveModel {
            id: Set(id),
            guild_id: Set(guild_id),
            legacy_id: Set(None),
            creator_user_id: Set(None),
            name: Set(name.to_owned()),
            normalized_name: Set(name.to_lowercase()),
            description: Set("No description provided".to_owned()),
            emoji: Set("🏆".to_owned()),
            value: Set(10),
            image: Set(None),
            dedication_user_id: Set(None),
            dedication_text: Set(None),
            details: Set("No details provided.".to_owned()),
            signed: Set(false),
            category: Set(None),
            active: Set(true),
            created_at: Set(now()),
            updated_at: Set(now()),
        }
        .insert(db)
        .await
        .expect("seed trophy");
        id
    }

    async fn seed_award(db: &DatabaseConnection, guild_id: i64, user_id: i64, trophy_id: Uuid) {
        user_trophies::ActiveModel {
            id: Set(Uuid::now_v7()),
            guild_id: Set(guild_id),
            user_id: Set(user_id),
            trophy_id: Set(trophy_id),
            awarded_by: Set(None),
            awarded_at: Set(now()),
            created_at: Set(now()),
            updated_at: Set(now()),
        }
        .insert(db)
        .await
        .expect("seed award");
    }

    async fn seed_counter(db: &DatabaseConnection, name: &str, total: i64) {
        bot_stats::ActiveModel {
            id: NotSet,
            name: Set(name.to_owned()),
            total: Set(total),
            created_at: Set(now()),
            updated_at: Set(now()),
        }
        .insert(db)
        .await
        .expect("seed counter");
    }

    async fn counter(db: &DatabaseConnection, name: &str) -> Option<i64> {
        bot_stats::Entity::find()
            .filter(bot_stats::Column::Name.eq(name))
            .one(db)
            .await
            .expect("read counter")
            .map(|row| row.total)
    }

    #[tokio::test]
    async fn snapshot_is_zero_on_empty_database() {
        let db = fresh_db().await;
        let snapshot = load_snapshot(&db).await.expect("load snapshot");
        assert_eq!(
            snapshot,
            StatsSnapshot { guilds: 0, trophies: 0, awards: 0, command_runs: 0 }
        );
    }

    #[tokio::test]
    async fn snapshot_counts_live_rows_and_reads_total_counter() {
        let db = fresh_db().await;
        insert_guild(&db, 100).await;
        insert_guild(&db, 200).await;
        let gold = seed_trophy(&db, 100, "Gold").await;
        seed_trophy(&db, 200, "Silver").await;
        // Duplicate awards of the same trophy are counted individually.
        seed_award(&db, 100, 500, gold).await;
        seed_award(&db, 100, 500, gold).await;
        seed_award(&db, 100, 501, gold).await;
        seed_counter(&db, TOTAL_COUNTER, 104_913).await;

        let snapshot = load_snapshot(&db).await.expect("load snapshot");
        assert_eq!(
            snapshot,
            StatsSnapshot { guilds: 2, trophies: 2, awards: 3, command_runs: 104_913 }
        );
    }

    #[tokio::test]
    async fn record_successful_run_creates_both_counters() {
        let db = fresh_db().await;
        record_successful_run(&db, "stats").await.expect("record run");
        record_successful_run(&db, "stats").await.expect("record run");

        assert_eq!(counter(&db, "total").await, Some(2));
        assert_eq!(counter(&db, "stats").await, Some(2));
    }

    #[tokio::test]
    async fn record_successful_run_continues_imported_historical_counters() {
        let db = fresh_db().await;
        seed_counter(&db, TOTAL_COUNTER, 104_913).await;
        seed_counter(&db, "stats", 766).await;

        record_successful_run(&db, "stats").await.expect("record run");

        assert_eq!(counter(&db, "total").await, Some(104_914));
        assert_eq!(counter(&db, "stats").await, Some(767));
        // Other imported counters are untouched.
        assert_eq!(counter(&db, "award").await, None);
    }

    #[tokio::test]
    async fn record_successful_run_nests_inside_caller_transaction() {
        let db = fresh_db().await;

        // A caller already inside a transaction gets a savepoint: rolling the
        // outer transaction back discards both counter bumps together, so the
        // `total` and per-command counters cannot drift apart.
        let outer = db.begin().await.expect("begin outer transaction");
        record_successful_run(&outer, "stats").await.expect("record run");
        outer.rollback().await.expect("rollback outer transaction");

        assert_eq!(counter(&db, "total").await, None);
        assert_eq!(counter(&db, "stats").await, None);

        // And committing persists both.
        let outer = db.begin().await.expect("begin outer transaction");
        record_successful_run(&outer, "stats").await.expect("record run");
        outer.commit().await.expect("commit outer transaction");

        assert_eq!(counter(&db, "total").await, Some(1));
        assert_eq!(counter(&db, "stats").await, Some(1));
    }

    #[test]
    fn uptime_parts_splits_days_hours_minutes_seconds() {
        assert_eq!(uptime_parts(Duration::ZERO), (0, 0, 0, 0));
        assert_eq!(uptime_parts(Duration::from_secs(59)), (0, 0, 0, 59));
        assert_eq!(uptime_parts(Duration::from_secs(3_661)), (0, 1, 1, 1));
        // 2d 3h 4m 5s
        assert_eq!(
            uptime_parts(Duration::from_secs(2 * 86_400 + 3 * 3_600 + 4 * 60 + 5)),
            (2, 3, 4, 5)
        );
    }

    #[test]
    fn stats_messages_exist_in_catalog() {
        let locale = i18n::resolve(None);
        assert_ne!(i18n::t(&locale, "stats-title"), "stats-title");
        assert_ne!(i18n::t(&locale, "stats-discord-label"), "stats-discord-label");
        assert_ne!(i18n::t(&locale, "stats-trophies-label"), "stats-trophies-label");

        // Fluent wraps placeables in bidi isolation marks; strip them so we
        // can assert on the plain "1d 1h 1m 1s" shape.
        let uptime: String = format_uptime(&locale, Duration::from_secs(90_061))
            .replace(['\u{2068}', '\u{2069}'], "");
        for part in ["1d", "1h", "1m", "1s"] {
            assert!(uptime.contains(part), "uptime missing {part}: {uptime}");
        }

        let discord = i18n::t_args(
            &locale,
            "stats-discord-value",
            &[("servers", 7.into()), ("users", 42.into()), ("uptime", "1d".into())],
        );
        assert!(discord.contains('7') && discord.contains("42"), "got: {discord}");

        let trophies = i18n::t_args(
            &locale,
            "stats-trophies-value",
            &[
                ("commands", 24.into()),
                ("runs", 104_913.into()),
                ("guilds", 2_493.into()),
                ("trophies", 10_853.into()),
                ("awarded", 60_554.into()),
            ],
        );
        assert!(trophies.contains("24"), "got: {trophies}");
    }
}
