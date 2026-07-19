//! `trophy-bot smoke` — the end-to-end smoke flow of implementation-plan
//! Phase 4 item 8: create → award ×3 → leaderboard → revoke → clear → panel,
//! executed against the REAL test guild (`TEST_GUILD_ID`) over Discord HTTP.
//!
//! Ground rules:
//! - DISPOSABLE database: `./smoke.sqlite` is created fresh (migrations run
//!   programmatically), the legacy `./json.sqlite` is imported into it via
//!   `crate::import::import_data`, and the file is deleted at the end.
//!   `rdb.sqlite` (the dev bot DB) is never touched.
//! - The SAME business-logic functions the slash commands use are driven
//!   directly: `create::{validate_fields,precheck,insert_trophy}`,
//!   `award::insert_awards`, `revoke::revoke_awards`, `clear::clear_awards`,
//!   `rewards::{clear_rewards,add_reward}`, `reward_apply::apply_rewards_via`,
//!   `render::render_leaderboard`, `panel_updater::{save_panel,refresh_panel}`
//!   and `domain::queries` — no logic is reimplemented here.
//! - Discord-side objects (temp role, temp channel) are cleaned up
//!   best-effort even when a step fails.
//! - Every step logs PASS/FAIL/SKIPPED; exit code 0 (and the final
//!   `SMOKE: ALL STEPS PASSED` line) only when every assertion held.
//!
//! This is a dev tool: its strings are log output and deliberately not i18n.

use std::collections::HashMap;

use anyhow::{bail, Context as _, Result};
use sea_orm::{
    ColumnTrait, ConnectOptions, Database, DatabaseConnection, EntityTrait, QueryFilter,
};
use sea_orm_migration::MigratorTrait;
use serenity::all::{
    ChannelId, ChannelType, CreateChannel, CreateMessage, EditRole, GuildId, Http, HttpError,
    MessageId, RoleId, StatusCode, UserId,
};
use uuid::Uuid;

use crate::bot::commands::award::insert_awards;
use crate::bot::commands::clear::clear_awards;
use crate::bot::commands::create::{self, NewTrophy};
use crate::bot::commands::revoke::revoke_awards;
use crate::bot::commands::rewards::{add_reward, clear_rewards, AddOutcome};
use crate::bot::panel_updater::{self, PanelFate};
use crate::bot::render;
use crate::bot::reward_apply::{self, bot_top_position, filter_assignable, RoleMeta};
use crate::cli::Cli;
use crate::domain::normalize::normalize_name;
use crate::domain::queries;
use crate::entities::{trophies, user_trophies};
use crate::i18n;
use crate::import::{self, ImportOptions};
use crate::legacy::{LegacyData, DEFAULT_LEGACY_DB_PATH};
use crate::migrations::Migrator;

const SMOKE_DB_FILE: &str = "./smoke.sqlite";
const SMOKE_DB_URL: &str = "sqlite://./smoke.sqlite?mode=rwc";
const TROPHY_NAME: &str = "Smoke Test Trophy";
const TROPHY_VALUE: i32 = 100;
const ROLE_NAME: &str = "smoke-reward";
const ROLE_REQUIREMENT: i32 = 150;
const CHANNEL_NAME: &str = "smoke-panel";

// ---------------------------------------------------------------------------
// Step bookkeeping
// ---------------------------------------------------------------------------

enum Outcome {
    Pass,
    Fail(String),
    Skipped(String),
}

#[derive(Default)]
struct Report {
    steps: Vec<(&'static str, Outcome)>,
}

impl Report {
    fn pass(&mut self, step: &'static str) {
        log::info!("SMOKE {step}: PASS");
        self.steps.push((step, Outcome::Pass));
    }

    fn fail(&mut self, step: &'static str, reason: impl Into<String>) -> anyhow::Error {
        let reason = reason.into();
        log::error!("SMOKE {step}: FAIL — {reason}");
        self.steps.push((step, Outcome::Fail(reason.clone())));
        anyhow::anyhow!("step {step} failed: {reason}")
    }

    fn skip(&mut self, step: &'static str, reason: impl Into<String>) {
        let reason = reason.into();
        log::warn!("SMOKE {step}: SKIPPED — {reason}");
        self.steps.push((step, Outcome::Skipped(reason)));
    }

    /// Logs the final per-step summary; `true` when no step failed.
    fn summarize(&self) -> bool {
        log::info!("SMOKE summary:");
        let mut ok = true;
        for (step, outcome) in &self.steps {
            match outcome {
                Outcome::Pass => log::info!("  {step}: PASS"),
                Outcome::Skipped(reason) => log::warn!("  {step}: SKIPPED ({reason})"),
                Outcome::Fail(reason) => {
                    ok = false;
                    log::error!("  {step}: FAIL ({reason})");
                }
            }
        }
        ok
    }
}

/// Discord objects (and the trophy row) to tear down at the end, no matter
/// how far the flow got.
#[derive(Default)]
struct Cleanup {
    role_id: Option<RoleId>,
    channel_id: Option<ChannelId>,
    trophy_id: Option<Uuid>,
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

pub async fn run(cli: &Cli) -> Result<()> {
    // Same rationale as `migrations::cli`: the harness reports through logs.
    if !cli.debug {
        log::set_max_level(log::LevelFilter::Info);
    }

    let guild_id = GuildId::new(
        cli.test_guild_id
            .context("TEST_GUILD_ID must be set — the smoke flow runs against the test guild")?,
    );
    let http = Http::from(cli);
    let bot_id = http
        .get_current_user()
        .await
        .context("token check failed: could not fetch the bot's own user")?
        .id;
    log::info!("smoke: bot user {bot_id}, test guild {guild_id}");

    let db = prepare_database().await?;

    let mut report = Report::default();
    let mut cleanup = Cleanup::default();
    let result = execute(&db, &http, guild_id, bot_id, &mut report, &mut cleanup).await;
    if let Err(err) = &result {
        log::error!("smoke: flow aborted: {err:#}");
    }

    run_cleanup(&db, &http, guild_id, &cleanup).await;
    if let Err(err) = db.close().await {
        log::warn!("smoke: could not close the smoke database cleanly: {err}");
    }
    remove_db_files();

    if report.summarize() && result.is_ok() {
        log::info!("SMOKE: ALL STEPS PASSED");
        Ok(())
    } else {
        bail!("smoke flow failed — see the step summary above");
    }
}

/// Fresh `./smoke.sqlite`: delete leftovers, run all migrations
/// programmatically, then import the legacy `./json.sqlite` through the real
/// importer so the flow exercises production-shaped data.
async fn prepare_database() -> Result<DatabaseConnection> {
    remove_db_files();

    let mut options = ConnectOptions::new(SMOKE_DB_URL);
    // Single connection: same reasoning as the test scaffolding — and the
    // harness is strictly sequential anyway.
    options.max_connections(1).sqlx_logging(false);
    let db = Database::connect(options)
        .await
        .context("connecting to the disposable smoke database")?;

    Migrator::fresh(&db)
        .await
        .context("running migrations on the smoke database")?;
    log::info!("smoke: migrations applied on {SMOKE_DB_FILE}");

    let legacy = LegacyData::load(DEFAULT_LEGACY_DB_PATH)
        .await
        .context("loading the legacy quick.db file")?;
    let import_report = import::import_data(&db, &legacy, &ImportOptions::default())
        .await
        .context("importing legacy data into the smoke database")?;
    log::info!(
        "smoke: import done — {} guilds, {} trophies, {} awards",
        import_report.guilds,
        import_report.trophies,
        import_report.awards_inserted
    );
    Ok(db)
}

fn remove_db_files() {
    for suffix in ["", "-wal", "-shm"] {
        let path = format!("{SMOKE_DB_FILE}{suffix}");
        match std::fs::remove_file(&path) {
            Ok(()) => log::debug!("smoke: removed {path}"),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => log::warn!("smoke: could not remove {path}: {err}"),
        }
    }
}

// ---------------------------------------------------------------------------
// The flow
// ---------------------------------------------------------------------------

async fn execute(
    db: &DatabaseConnection,
    http: &Http,
    guild_id: GuildId,
    bot_id: UserId,
    report: &mut Report,
    cleanup: &mut Cleanup,
) -> Result<()> {
    let gid = guild_id.get() as i64;
    let guild = guild_id
        .to_partial_guild(http)
        .await
        .context("fetching the test guild")?;

    // --- CREATE ------------------------------------------------------------
    let trophy = step_create(db, gid, bot_id, report).await?;
    cleanup.trophy_id = Some(trophy.id);

    // --- REWARD SETUP --------------------------------------------------------
    let role_id = step_reward_setup(db, http, guild_id, report, cleanup).await?;

    // --- AWARD ×3 ------------------------------------------------------------
    let target =
        step_award(db, http, guild_id, bot_id, guild.owner_id, trophy.id, role_id, report).await?;

    // --- LEADERBOARD ---------------------------------------------------------
    step_leaderboard(db, http, guild_id, &guild.name, target, report).await?;

    // --- REVOKE ×1 -----------------------------------------------------------
    step_revoke(db, http, guild_id, bot_id, target, trophy.id, role_id, report).await?;

    // --- CLEAR ---------------------------------------------------------------
    step_clear(db, http, guild_id, bot_id, target, role_id, report).await?;

    // --- PANEL ---------------------------------------------------------------
    step_panel(db, http, guild_id, &guild.name, bot_id, target, trophy.id, report, cleanup)
        .await?;

    Ok(())
}

/// CREATE: same validate → precheck → insert pipeline as `/create` (F3/F5).
async fn step_create(
    db: &DatabaseConnection,
    gid: i64,
    bot_id: UserId,
    report: &mut Report,
) -> Result<trophies::Model> {
    const STEP: &str = "CREATE";

    if let Err(err) =
        create::validate_fields(TROPHY_NAME, None, None, TROPHY_VALUE, None, None)
    {
        return Err(report.fail(STEP, format!("field validation rejected: {err:?}")));
    }
    match create::precheck(db, gid, TROPHY_NAME).await {
        Ok(None) => {}
        Ok(Some(err)) => {
            return Err(report.fail(STEP, format!("uniqueness/capacity precheck rejected: {err:?}")));
        }
        Err(err) => return Err(report.fail(STEP, format!("precheck query failed: {err:#}"))),
    }

    let new = NewTrophy {
        id: Uuid::now_v7(),
        guild_id: gid,
        creator_user_id: bot_id.get() as i64,
        name: TROPHY_NAME.to_string(),
        description: "Created by the smoke harness".to_string(),
        emoji: "🏆".to_string(),
        value: TROPHY_VALUE,
        image: None,
        dedication_user_id: None,
        dedication_text: None,
        details: "Disposable smoke-test trophy.".to_string(),
        signed: false,
        category: None,
        active: true,
    };
    let trophy = match create::insert_trophy(db, new).await {
        Ok(Ok(model)) => model,
        Ok(Err(err)) => return Err(report.fail(STEP, format!("insert rejected: {err:?}"))),
        Err(err) => return Err(report.fail(STEP, format!("insert failed: {err:#}"))),
    };

    // Assert the row is really there, with the ADR 0005 normalized name.
    let stored = match trophies::Entity::find_by_id(trophy.id).one(db).await {
        Ok(Some(model)) => model,
        Ok(None) => return Err(report.fail(STEP, "inserted trophy row not found")),
        Err(err) => return Err(report.fail(STEP, format!("row lookup failed: {err:#}"))),
    };
    let expected = normalize_name(TROPHY_NAME);
    if stored.normalized_name != expected {
        return Err(report.fail(
            STEP,
            format!("normalized_name is `{}`, expected `{expected}`", stored.normalized_name),
        ));
    }

    report.pass(STEP);
    Ok(trophy)
}

/// REWARD SETUP: temp Discord role + `role_rewards` row (requirement 150),
/// through the same `add_reward` the `/rewards add` handler uses. The guild's
/// imported legacy rewards are cleared first so the engine's target set is
/// deterministic (disposable DB — nothing real is lost).
async fn step_reward_setup(
    db: &DatabaseConnection,
    http: &Http,
    guild_id: GuildId,
    report: &mut Report,
    cleanup: &mut Cleanup,
) -> Result<Option<RoleId>> {
    const STEP: &str = "REWARD SETUP";
    let gid = guild_id.get() as i64;

    match clear_rewards(db, gid).await {
        Ok(outcome) => log::info!("smoke: cleared imported rewards for determinism: {outcome:?}"),
        Err(err) => return Err(report.fail(STEP, format!("clearing imported rewards: {err:#}"))),
    }

    let role = match guild_id.create_role(http, EditRole::new().name(ROLE_NAME)).await {
        Ok(role) => role,
        Err(err) if is_missing_permission(&err) => {
            report.skip(STEP, format!("bot lacks Manage Roles in the test guild: {err}"));
            return Ok(None);
        }
        Err(err) => return Err(report.fail(STEP, format!("creating the temp role: {err}"))),
    };
    cleanup.role_id = Some(role.id);
    log::info!("smoke: created temp role {} ({})", ROLE_NAME, role.id);

    match add_reward(db, gid, role.id.get() as i64, ROLE_REQUIREMENT).await {
        Ok(AddOutcome::Added) => {}
        Ok(other) => return Err(report.fail(STEP, format!("add_reward returned {other:?}"))),
        Err(err) => return Err(report.fail(STEP, format!("add_reward failed: {err:#}"))),
    }

    report.pass(STEP);
    Ok(Some(role.id))
}

/// AWARD ×3 to the guild owner (bot's own user as the hierarchy fallback):
/// 3 rows with `awarded_by`, recomputed score 300, and — via the real reward
/// engine — the smoke-reward role actually present on the member.
#[allow(clippy::too_many_arguments)]
async fn step_award(
    db: &DatabaseConnection,
    http: &Http,
    guild_id: GuildId,
    bot_id: UserId,
    owner_id: UserId,
    trophy_id: Uuid,
    role_id: Option<RoleId>,
    report: &mut Report,
) -> Result<UserId> {
    const STEP_DB: &str = "AWARD x3 (db)";
    const STEP_ROLE: &str = "AWARD x3 (role assigned on Discord)";
    let gid = guild_id.get() as i64;

    let mut target = owner_id;
    award_db_side(db, gid, target, trophy_id, bot_id).await.map_err(|e| report.fail(STEP_DB, format!("{e:#}")))?;
    report.pass(STEP_DB);

    let Some(role_id) = role_id else {
        report.skip(STEP_ROLE, "no reward role (see REWARD SETUP)");
        return Ok(target);
    };

    apply_engine(db, http, guild_id, bot_id, target).await.map_err(|e| report.fail(STEP_ROLE, format!("{e:#}")))?;
    if member_has_role(http, guild_id, target, role_id).await.map_err(|e| report.fail(STEP_ROLE, format!("{e:#}")))? {
        report.pass(STEP_ROLE);
        return Ok(target);
    }

    // Fallback: the owner did not get the role — retry against the bot's own
    // member, then diagnose hierarchy before failing.
    log::warn!(
        "smoke: reward role not on the guild owner after apply; falling back to the bot's own user"
    );
    clear_awards(db, gid, target.get() as i64)
        .await
        .map_err(|e| report.fail(STEP_ROLE, format!("cleanup of owner awards: {e:#}")))?;
    target = bot_id;
    award_db_side(db, gid, target, trophy_id, bot_id).await.map_err(|e| report.fail(STEP_ROLE, format!("{e:#}")))?;
    apply_engine(db, http, guild_id, bot_id, target).await.map_err(|e| report.fail(STEP_ROLE, format!("{e:#}")))?;
    if member_has_role(http, guild_id, target, role_id).await.map_err(|e| report.fail(STEP_ROLE, format!("{e:#}")))? {
        report.pass(STEP_ROLE);
        return Ok(target);
    }

    match diagnose_hierarchy(http, guild_id, bot_id, role_id).await {
        Ok(Some(reason)) => {
            report.skip(STEP_ROLE, reason);
            Ok(target)
        }
        Ok(None) => Err(report.fail(
            STEP_ROLE,
            "role not assigned on Discord and no hierarchy problem detected",
        )),
        Err(err) => Err(report.fail(STEP_ROLE, format!("hierarchy diagnosis failed: {err:#}"))),
    }
}

/// The DB half of the award step: deterministic baseline (clear the target's
/// imported awards), 3 awards via the shared `insert_awards`, then assert the
/// rows and the recomputed score.
async fn award_db_side(
    db: &DatabaseConnection,
    gid: i64,
    target: UserId,
    trophy_id: Uuid,
    awarded_by: UserId,
) -> Result<()> {
    let uid = target.get() as i64;
    let baseline = clear_awards(db, gid, uid)
        .await
        .context("clearing the target user's imported awards for a zero baseline")?;
    if baseline > 0 {
        log::info!("smoke: cleared {baseline} imported award(s) of user {uid} for determinism");
    }

    insert_awards(db, gid, uid, trophy_id, 3, awarded_by.get() as i64)
        .await
        .context("insert_awards")?;

    let rows = user_trophies::Entity::find()
        .filter(user_trophies::Column::GuildId.eq(gid))
        .filter(user_trophies::Column::UserId.eq(uid))
        .filter(user_trophies::Column::TrophyId.eq(trophy_id))
        .all(db)
        .await
        .context("reading back award rows")?;
    if rows.len() != 3 {
        bail!("expected 3 award rows, found {}", rows.len());
    }
    if let Some(row) = rows.iter().find(|row| row.awarded_by != Some(awarded_by.get() as i64)) {
        bail!("award row {} has awarded_by {:?}, expected {}", row.id, row.awarded_by, awarded_by);
    }

    let score = queries::user_score(db, gid, uid).await.context("recomputing the score")?;
    if score != 300 {
        bail!("score after 3 awards is {score}, expected 300");
    }
    Ok(())
}

/// LEADERBOARD: domain query + the shared renderer (same path as
/// `/leaderboard` and the panel updater). The target user must show up with
/// score 300 both in the query and in the rendered embed.
async fn step_leaderboard(
    db: &DatabaseConnection,
    http: &Http,
    guild_id: GuildId,
    guild_name: &str,
    target: UserId,
    report: &mut Report,
) -> Result<()> {
    const STEP: &str = "LEADERBOARD";
    let gid = guild_id.get() as i64;
    let uid = target.get() as i64;

    let board = queries::leaderboard(db, gid)
        .await
        .map_err(|e| report.fail(STEP, format!("leaderboard query failed: {e:#}")))?;
    if !board.iter().any(|&(user, score)| user == uid && score == 300) {
        return Err(report.fail(
            STEP,
            format!("domain query does not list user {uid} with score 300: {board:?}"),
        ));
    }

    // Board is small (test guild), so the target is on page 1; the imported
    // `leaderboard_format` setting is Username, so match either the mention
    // or the resolved username.
    let locale = i18n::resolve(None);
    let embed = render::render_leaderboard(db, http, guild_id, guild_name, 1, &locale, true)
        .await
        .map_err(|e| report.fail(STEP, format!("shared renderer failed: {e:#}")))?;
    let json = serde_json::to_value(&embed)
        .map_err(|e| report.fail(STEP, format!("embed serialization failed: {e}")))?;
    let rows = json["fields"][0]["value"].as_str().unwrap_or_default().to_string();

    let username = guild_id
        .member(http, target)
        .await
        .map(|member| member.user.name.to_string())
        .unwrap_or_default();
    let named = !username.is_empty() && rows.contains(&username);
    let mentioned = rows.contains(&format!("<@{uid}>"));
    if !(named || mentioned) {
        return Err(report.fail(STEP, format!("rendered rows do not show the target user: {rows}")));
    }
    if !rows.contains("300") {
        return Err(report.fail(STEP, format!("rendered rows do not show score 300: {rows}")));
    }

    report.pass(STEP);
    Ok(())
}

/// REVOKE ×1 of the requested trophy: exactly one row of THAT trophy gone
/// (most recent first — `revoke_awards` guarantees it), score 200, and the
/// reward role still held (200 >= 150).
#[allow(clippy::too_many_arguments)]
async fn step_revoke(
    db: &DatabaseConnection,
    http: &Http,
    guild_id: GuildId,
    bot_id: UserId,
    target: UserId,
    trophy_id: Uuid,
    role_id: Option<RoleId>,
    report: &mut Report,
) -> Result<()> {
    const STEP_DB: &str = "REVOKE x1 (db)";
    const STEP_ROLE: &str = "REVOKE (role still held)";
    let gid = guild_id.get() as i64;
    let uid = target.get() as i64;

    let removed = revoke_awards(db, gid, uid, trophy_id, 1)
        .await
        .map_err(|e| report.fail(STEP_DB, format!("revoke_awards failed: {e:#}")))?;
    if removed != 1 {
        return Err(report.fail(STEP_DB, format!("removed {removed} rows, expected exactly 1")));
    }
    let remaining = user_trophies::Entity::find()
        .filter(user_trophies::Column::GuildId.eq(gid))
        .filter(user_trophies::Column::UserId.eq(uid))
        .filter(user_trophies::Column::TrophyId.eq(trophy_id))
        .all(db)
        .await
        .map_err(|e| report.fail(STEP_DB, format!("reading remaining rows: {e:#}")))?;
    if remaining.len() != 2 {
        return Err(report.fail(STEP_DB, format!("{} rows remain, expected 2", remaining.len())));
    }
    let score = queries::user_score(db, gid, uid)
        .await
        .map_err(|e| report.fail(STEP_DB, format!("score query failed: {e:#}")))?;
    if score != 200 {
        return Err(report.fail(STEP_DB, format!("score after revoke is {score}, expected 200")));
    }
    report.pass(STEP_DB);

    let Some(role_id) = role_id else {
        report.skip(STEP_ROLE, "no reward role (see REWARD SETUP)");
        return Ok(());
    };
    apply_engine(db, http, guild_id, bot_id, target)
        .await
        .map_err(|e| report.fail(STEP_ROLE, format!("{e:#}")))?;
    if member_has_role(http, guild_id, target, role_id)
        .await
        .map_err(|e| report.fail(STEP_ROLE, format!("{e:#}")))?
    {
        report.pass(STEP_ROLE);
    } else {
        return Err(report.fail(STEP_ROLE, "role was removed although 200 >= 150"));
    }
    Ok(())
}

/// CLEAR: all awards gone, score 0, and the reward role REALLY removed.
async fn step_clear(
    db: &DatabaseConnection,
    http: &Http,
    guild_id: GuildId,
    bot_id: UserId,
    target: UserId,
    role_id: Option<RoleId>,
    report: &mut Report,
) -> Result<()> {
    const STEP_DB: &str = "CLEAR (db)";
    const STEP_ROLE: &str = "CLEAR (role removed on Discord)";
    let gid = guild_id.get() as i64;
    let uid = target.get() as i64;

    let cleared = clear_awards(db, gid, uid)
        .await
        .map_err(|e| report.fail(STEP_DB, format!("clear_awards failed: {e:#}")))?;
    if cleared != 2 {
        return Err(report.fail(STEP_DB, format!("cleared {cleared} rows, expected 2")));
    }
    let score = queries::user_score(db, gid, uid)
        .await
        .map_err(|e| report.fail(STEP_DB, format!("score query failed: {e:#}")))?;
    if score != 0 {
        return Err(report.fail(STEP_DB, format!("score after clear is {score}, expected 0")));
    }
    report.pass(STEP_DB);

    let Some(role_id) = role_id else {
        report.skip(STEP_ROLE, "no reward role (see REWARD SETUP)");
        return Ok(());
    };
    apply_engine(db, http, guild_id, bot_id, target)
        .await
        .map_err(|e| report.fail(STEP_ROLE, format!("{e:#}")))?;
    if member_has_role(http, guild_id, target, role_id)
        .await
        .map_err(|e| report.fail(STEP_ROLE, format!("{e:#}")))?
    {
        return Err(report.fail(STEP_ROLE, "role still held although the score is 0"));
    }
    report.pass(STEP_ROLE);
    Ok(())
}

/// PANEL: temp channel, panel message via the shared renderer, record saved
/// with `save_panel` (F31 path), a real score change, then one pass through
/// `refresh_panel` — the message must exist and have been edited.
#[allow(clippy::too_many_arguments)]
async fn step_panel(
    db: &DatabaseConnection,
    http: &Http,
    guild_id: GuildId,
    guild_name: &str,
    bot_id: UserId,
    target: UserId,
    trophy_id: Uuid,
    report: &mut Report,
    cleanup: &mut Cleanup,
) -> Result<()> {
    const STEP: &str = "PANEL";
    let gid = guild_id.get() as i64;

    let channel = match guild_id
        .create_channel(http, CreateChannel::new(CHANNEL_NAME).kind(ChannelType::Text))
        .await
    {
        Ok(channel) => channel,
        Err(err) if is_missing_permission(&err) => {
            report.skip(STEP, format!("bot lacks Manage Channels in the test guild: {err}"));
            return Ok(());
        }
        Err(err) => return Err(report.fail(STEP, format!("creating the temp channel: {err}"))),
    };
    cleanup.channel_id = Some(channel.id);
    log::info!("smoke: created temp channel #{} ({})", CHANNEL_NAME, channel.id);

    // Same render path and locale policy as `/panel create`.
    let locale = i18n::resolve(None);
    let embed = render::render_leaderboard(db, http, guild_id, guild_name, 1, &locale, false)
        .await
        .map_err(|e| report.fail(STEP, format!("panel render failed: {e:#}")))?;
    let message = channel
        .id
        .send_message(http, CreateMessage::new().embed(embed))
        .await
        .map_err(|e| report.fail(STEP, format!("sending the panel message: {e}")))?;
    panel_updater::save_panel(db, gid, channel.id.get() as i64, message.id.get() as i64, None)
        .await
        .map_err(|e| report.fail(STEP, format!("save_panel failed: {e:#}")))?;

    // A real score change so the refreshed embed differs from the posted one.
    insert_awards(db, gid, target.get() as i64, trophy_id, 1, bot_id.get() as i64)
        .await
        .map_err(|e| report.fail(STEP, format!("score change before refresh: {e:#}")))?;

    let panel = panel_updater::get_panel(db, gid)
        .await
        .map_err(|e| report.fail(STEP, format!("get_panel failed: {e:#}")))?
        .ok_or_else(|| report.fail(STEP, "panel record missing after save_panel"))?;
    let fate = panel_updater::refresh_panel(db, http, &panel)
        .await
        .map_err(|e| report.fail(STEP, format!("refresh_panel failed: {e:#}")))?;
    if fate != PanelFate::Updated {
        return Err(report.fail(STEP, format!("refresh_panel returned {fate:?}, expected Updated")));
    }

    let fetched = channel
        .id
        .message(http, MessageId::new(message.id.get()))
        .await
        .map_err(|e| report.fail(STEP, format!("fetching the panel message back: {e}")))?;
    if fetched.embeds.is_empty() {
        return Err(report.fail(STEP, "refreshed panel message has no embed"));
    }
    if fetched.edited_timestamp.is_none() {
        return Err(report.fail(STEP, "panel message was never edited (no edited_timestamp)"));
    }

    report.pass(STEP);
    Ok(())
}

// ---------------------------------------------------------------------------
// Discord helpers
// ---------------------------------------------------------------------------

/// Runs the real reward engine (same function `/award`, `/revoke` and
/// `/clear` call) over plain HTTP.
async fn apply_engine(
    db: &DatabaseConnection,
    http: &Http,
    guild_id: GuildId,
    bot_id: UserId,
    user_id: UserId,
) -> Result<()> {
    reward_apply::apply_rewards_via(db, http, bot_id, None, guild_id, user_id)
        .await
        .context("reward engine (apply_rewards_via)")
}

async fn member_has_role(
    http: &Http,
    guild_id: GuildId,
    user_id: UserId,
    role_id: RoleId,
) -> Result<bool> {
    let member = guild_id
        .member(http, user_id)
        .await
        .with_context(|| format!("fetching member {user_id} of guild {guild_id}"))?;
    Ok(member.roles.contains(&role_id))
}

/// `Some(reason)` when the temp role is genuinely unassignable for the bot
/// (at/above its top role, or managed) — the honest SKIPPED case.
async fn diagnose_hierarchy(
    http: &Http,
    guild_id: GuildId,
    bot_id: UserId,
    role_id: RoleId,
) -> Result<Option<String>> {
    let roles = guild_id.roles(http).await.context("fetching guild roles")?;
    let meta: HashMap<i64, RoleMeta> = roles
        .iter()
        .map(|(id, role)| {
            (id.get() as i64, RoleMeta { position: role.position, managed: role.managed })
        })
        .collect();
    let bot_member = guild_id
        .member(http, bot_id)
        .await
        .context("fetching the bot's own member")?;
    let bot_roles: Vec<i64> = bot_member.roles.iter().map(|id| id.get() as i64).collect();
    let bot_top = bot_top_position(&bot_roles, &meta);
    let (_, skipped) = filter_assignable(&[role_id.get() as i64], &meta, bot_top);
    if skipped.is_empty() {
        return Ok(None);
    }
    Ok(Some(format!(
        "role {role_id} is not assignable by the bot (position {:?} vs bot top {bot_top}) — \
         missing role hierarchy, not a logic failure",
        meta.get(&(role_id.get() as i64)).map(|m| m.position)
    )))
}

/// Discord answered 403: the bot is missing a permission — the honest
/// SKIPPED trigger for Discord-side steps.
fn is_missing_permission(error: &serenity::Error) -> bool {
    matches!(
        error,
        serenity::Error::Http(HttpError::UnsuccessfulRequest(response))
            if response.status_code == StatusCode::FORBIDDEN
    )
}

// ---------------------------------------------------------------------------
// Cleanup (best effort, always runs)
// ---------------------------------------------------------------------------

async fn run_cleanup(db: &DatabaseConnection, http: &Http, guild_id: GuildId, cleanup: &Cleanup) {
    if let Some(channel_id) = cleanup.channel_id {
        match channel_id.delete(http).await {
            Ok(_) => log::info!("smoke cleanup: deleted temp channel {channel_id}"),
            Err(err) => log::warn!("smoke cleanup: could not delete channel {channel_id}: {err}"),
        }
    }
    if let Some(role_id) = cleanup.role_id {
        match guild_id.delete_role(http, role_id).await {
            Ok(()) => log::info!("smoke cleanup: deleted temp role {role_id}"),
            Err(err) => log::warn!("smoke cleanup: could not delete role {role_id}: {err}"),
        }
    }
    if let Some(trophy_id) = cleanup.trophy_id {
        // The whole DB file is deleted right after, but remove the created
        // trophy (and its award rows) explicitly as the flow promises.
        let awards = user_trophies::Entity::delete_many()
            .filter(user_trophies::Column::TrophyId.eq(trophy_id))
            .exec(db)
            .await;
        let trophy = trophies::Entity::delete_by_id(trophy_id).exec(db).await;
        match (awards, trophy) {
            (Ok(_), Ok(_)) => log::info!("smoke cleanup: deleted the smoke trophy row"),
            (awards, trophy) => log::warn!(
                "smoke cleanup: trophy row cleanup issue (awards: {awards:?}, trophy: {trophy:?})"
            ),
        }
    }
}
