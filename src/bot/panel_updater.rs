//! Background leaderboard-panel updater and the panel persistence layer.
//!
//! Spec: docs/specs/commands-admin.md §/panel "Rust target". Fixes owned
//! here (rust-parity-plan §3):
//! - F29: panels refresh **event-driven** — `/award`, `/revoke` and `/clear`
//!   call [`PanelSignal::notify`], and a per-guild debounce coalesces bursts
//!   (a 50-copy bulk award triggers ONE re-render). A low-frequency
//!   reconciliation sweep (~15 min) still walks every panel so drift from
//!   missed signals or restarts heals itself — unlike the legacy loop
//!   (60 s + 1 s/guild ≈ 42 min full cycle at production scale) it is a
//!   safety net, not the primary mechanism.
//! - F31: [`save_panel`] is only called by `/panel create` AFTER the panel
//!   message was successfully sent — no record ever points at a stub.
//! - F32: any refresh (debounced or sweep) that hits a **404** (unknown
//!   channel/message) deletes the record ([`settle_refresh`]) — day one the
//!   import brings 461 rows, many pointing at long-dead messages, and the
//!   first sweep clears them out.
//!
//! Rendering goes through the shared `crate::bot::render` path (same code
//! as `/leaderboard`, F13-F16): page 1, no footer, default locale — panels
//! are guild-public content with no interaction locale to inherit.
//!
//! Lifecycle (ADR 0009): the task is spawned by `Bot::new`, stops when the
//! shutdown `watch` channel flips (or every sender is dropped), and
//! `Bot::run` joins it after the shards are shut down.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use poise::serenity_prelude as serenity;
use sea_orm::sea_query::OnConflict;
use sea_orm::{
    ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder, Set, TransactionTrait,
};
use tokio::sync::{mpsc, watch};

use crate::bot::render;
use crate::entities::{guilds, leaderboard_panels};
use crate::i18n::{self, LanguageIdentifier};

/// Debounce window per guild: a burst of awards inside this window causes a
/// single refresh, and a steady stream still refreshes once per window
/// (the first signal fixes the deadline; later ones coalesce into it).
pub(crate) const DEBOUNCE: Duration = Duration::from_secs(5);

/// Low-frequency reconciliation sweep interval (F29/F32).
pub(crate) const SWEEP_INTERVAL: Duration = Duration::from_secs(15 * 60);

/// Delay before the FIRST sweep after startup — soon enough to clean the
/// imported stale rows on day one, late enough to let the gateway settle.
const INITIAL_SWEEP_DELAY: Duration = Duration::from_secs(60);

/// Pause between guilds inside a sweep (rate-limit friendliness; the legacy
/// loop used the same 1 s spacing).
const SWEEP_PACING: Duration = Duration::from_secs(1);

// ---------------------------------------------------------------------------
// Signal handle (lives on `Data`)
// ---------------------------------------------------------------------------

/// Cheap cloneable handle used by score-changing commands to request a
/// debounced panel refresh for a guild (F29).
#[derive(Clone)]
pub struct PanelSignal(mpsc::UnboundedSender<i64>);

impl PanelSignal {
    /// Fire-and-forget: never blocks a command handler. A send failure only
    /// happens when the updater task is gone (shutdown), so it is logged
    /// and swallowed.
    pub fn notify(&self, guild_id: i64) {
        if self.0.send(guild_id).is_err() {
            log::warn!("Panel updater not running; dropped refresh signal for guild {guild_id}");
        }
    }
}

/// Creates the signal channel pair: the sender goes into `Data`, the
/// receiver into [`run`].
pub fn signal_channel() -> (PanelSignal, mpsc::UnboundedReceiver<i64>) {
    let (tx, rx) = mpsc::unbounded_channel();
    (PanelSignal(tx), rx)
}

/// Cache + HTTP pair for Discord calls outside an interaction context.
/// `Bot::new` builds it from the serenity client's shared handles.
pub struct CacheAndHttp {
    pub cache: Arc<serenity::Cache>,
    pub http: Arc<serenity::Http>,
}

impl serenity::CacheHttp for CacheAndHttp {
    fn http(&self) -> &serenity::Http {
        &self.http
    }

    fn cache(&self) -> Option<&Arc<serenity::Cache>> {
        Some(&self.cache)
    }
}

// ---------------------------------------------------------------------------
// Debounce queue (pure, testable)
// ---------------------------------------------------------------------------

/// Per-guild debounce bookkeeping. A guild's FIRST signal fixes its
/// deadline at `now + delay`; further signals before the deadline coalesce
/// (they do NOT push the deadline out, so a constant stream of awards still
/// refreshes the panel once per window instead of starving it).
pub(crate) struct DebounceQueue {
    delay: Duration,
    pending: HashMap<i64, Instant>,
}

impl DebounceQueue {
    pub(crate) fn new(delay: Duration) -> Self {
        Self { delay, pending: HashMap::new() }
    }

    /// Records a refresh request for `guild_id` observed at `now`.
    pub(crate) fn signal(&mut self, guild_id: i64, now: Instant) {
        self.pending.entry(guild_id).or_insert(now + self.delay);
    }

    /// The earliest pending deadline, if any — what the run loop sleeps on.
    pub(crate) fn next_deadline(&self) -> Option<Instant> {
        self.pending.values().min().copied()
    }

    /// Removes and returns every guild whose deadline has passed.
    pub(crate) fn take_due(&mut self, now: Instant) -> Vec<i64> {
        let due: Vec<i64> = self
            .pending
            .iter()
            .filter(|(_, deadline)| **deadline <= now)
            .map(|(guild_id, _)| *guild_id)
            .collect();
        for guild_id in &due {
            self.pending.remove(guild_id);
        }
        due
    }
}

// ---------------------------------------------------------------------------
// Persistence helpers (leaderboard_panels table)
// ---------------------------------------------------------------------------

/// The guild's panel record, if one exists (one per guild — PK).
pub(crate) async fn get_panel(
    db: &DatabaseConnection,
    guild_id: i64,
) -> anyhow::Result<Option<leaderboard_panels::Model>> {
    Ok(leaderboard_panels::Entity::find_by_id(guild_id).one(db).await?)
}

/// Every panel record, in stable guild order (sweep input).
pub(crate) async fn all_panels(
    db: &DatabaseConnection,
) -> anyhow::Result<Vec<leaderboard_panels::Model>> {
    Ok(leaderboard_panels::Entity::find()
        .order_by_asc(leaderboard_panels::Column::GuildId)
        .all(db)
        .await?)
}

/// Records the panel message for a guild, replacing any previous record
/// (one panel per guild — PK upsert). Auto-registers the guild row so the
/// foreign key holds without clobbering an existing row.
///
/// F31: callers MUST invoke this only after the Discord message was
/// successfully sent.
pub(crate) async fn save_panel(
    db: &DatabaseConnection,
    guild_id: i64,
    channel_id: i64,
    message_id: i64,
) -> anyhow::Result<()> {
    let now = chrono::Utc::now().naive_utc();
    let txn = db.begin().await?;

    guilds::Entity::insert(guilds::ActiveModel {
        id: Set(guild_id),
        is_safe: Set(false),
        created_at: Set(now),
        updated_at: Set(now),
    })
    .on_conflict(OnConflict::column(guilds::Column::Id).do_nothing().to_owned())
    .exec_without_returning(&txn)
    .await?;

    leaderboard_panels::Entity::insert(leaderboard_panels::ActiveModel {
        guild_id: Set(guild_id),
        channel_id: Set(channel_id),
        message_id: Set(message_id),
        created_at: Set(now),
        updated_at: Set(now),
    })
    .on_conflict(
        OnConflict::column(leaderboard_panels::Column::GuildId)
            .update_columns([
                leaderboard_panels::Column::ChannelId,
                leaderboard_panels::Column::MessageId,
                leaderboard_panels::Column::UpdatedAt,
            ])
            .to_owned(),
    )
    .exec_without_returning(&txn)
    .await?;

    txn.commit().await?;
    Ok(())
}

/// Deletes the guild's panel record. Returns whether one existed.
pub(crate) async fn remove_panel(db: &DatabaseConnection, guild_id: i64) -> anyhow::Result<bool> {
    let result = leaderboard_panels::Entity::delete_by_id(guild_id).exec(db).await?;
    Ok(result.rows_affected > 0)
}

/// Marks a successful render (`updated_at` doubles as "last successful
/// render" for the sweep). A vanished row is a no-op, not an error.
pub(crate) async fn touch_panel(db: &DatabaseConnection, guild_id: i64) -> anyhow::Result<()> {
    leaderboard_panels::Entity::update_many()
        .set(leaderboard_panels::ActiveModel {
            updated_at: Set(chrono::Utc::now().naive_utc()),
            ..Default::default()
        })
        .filter(leaderboard_panels::Column::GuildId.eq(guild_id))
        .exec(db)
        .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Refresh outcome handling (F32)
// ---------------------------------------------------------------------------

/// What a refresh attempt did to the panel record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PanelFate {
    /// The guild has no panel record — nothing to do.
    NoPanel,
    /// Message edited; `updated_at` bumped.
    Updated,
    /// The target channel/message is gone (404) — record removed (F32).
    RemovedDead,
    /// Transient failure (network, permissions) — record kept for retry.
    KeptAfterError,
}

/// How an edit attempt failed, already classified.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EditFailure {
    /// Discord answered 404: the channel or message no longer exists.
    DeadTarget,
    /// Anything else — assumed recoverable, never destroys the record.
    Transient,
}

/// Only a definitive "does not exist" makes a target dead. 403 and friends
/// stay transient: permissions can be fixed, so the record must survive.
pub(crate) fn dead_status(status: serenity::StatusCode) -> bool {
    status == serenity::StatusCode::NOT_FOUND
}

/// Classifies a serenity error for [`settle_refresh`].
pub(crate) fn classify_error(error: &serenity::Error) -> EditFailure {
    match error {
        serenity::Error::Http(serenity::HttpError::UnsuccessfulRequest(response))
            if dead_status(response.status_code) =>
        {
            EditFailure::DeadTarget
        }
        _ => EditFailure::Transient,
    }
}

/// Applies an edit attempt's outcome to the DB: success bumps
/// `updated_at`, a dead target removes the record (F32), a transient
/// failure keeps it untouched for the next signal/sweep.
pub(crate) async fn settle_refresh(
    db: &DatabaseConnection,
    guild_id: i64,
    outcome: Result<(), EditFailure>,
) -> anyhow::Result<PanelFate> {
    match outcome {
        Ok(()) => {
            touch_panel(db, guild_id).await?;
            Ok(PanelFate::Updated)
        }
        Err(EditFailure::DeadTarget) => {
            remove_panel(db, guild_id).await?;
            log::info!("Removed panel record for guild {guild_id}: target channel/message is gone (F32)");
            Ok(PanelFate::RemovedDead)
        }
        Err(EditFailure::Transient) => Ok(PanelFate::KeptAfterError),
    }
}

// ---------------------------------------------------------------------------
// Rendering + Discord glue
// ---------------------------------------------------------------------------

/// Guild display name for the panel title: cache first, one HTTP fetch as
/// fallback, localized placeholder if both fail (never aborts a refresh).
async fn guild_display_name(
    cache_http: &impl serenity::CacheHttp,
    guild_id: serenity::GuildId,
    locale: &LanguageIdentifier,
) -> String {
    if let Some(cache) = cache_http.cache() {
        if let Some(guild) = cache.guild(guild_id) {
            return guild.name.to_string();
        }
    }
    match guild_id.to_partial_guild(cache_http).await {
        Ok(guild) => guild.name.to_string(),
        Err(error) => {
            log::debug!("Could not resolve name of guild {guild_id} for its panel: {error}");
            i18n::t(locale, "leaderboard-guild-fallback")
        }
    }
}

/// Best-effort deletion of a panel's Discord message (F30). Failures are
/// logged, never propagated — the caller proceeds either way.
pub(crate) async fn delete_panel_message(
    http: impl AsRef<serenity::Http>,
    panel: &leaderboard_panels::Model,
) {
    let (Ok(channel_id), Ok(message_id)) =
        (u64::try_from(panel.channel_id), u64::try_from(panel.message_id))
    else {
        return; // Corrupt ids cannot address a real message.
    };
    if let Err(error) = serenity::ChannelId::new(channel_id)
        .delete_message(http, serenity::MessageId::new(message_id))
        .await
    {
        log::warn!(
            "Could not delete panel message (guild={}, channel={channel_id}, message={message_id}): {error}",
            panel.guild_id
        );
    }
}

/// Re-renders one panel record and edits its message, then settles the
/// record per the outcome. Render errors (DB) propagate; Discord errors are
/// classified and settled (F32).
pub(crate) async fn refresh_panel(
    db: &DatabaseConnection,
    cache_http: &impl serenity::CacheHttp,
    panel: &leaderboard_panels::Model,
) -> anyhow::Result<PanelFate> {
    let (Ok(guild), Ok(channel), Ok(message)) = (
        u64::try_from(panel.guild_id),
        u64::try_from(panel.channel_id),
        u64::try_from(panel.message_id),
    ) else {
        // Ids that cannot be snowflakes can never resolve: dead by definition.
        return settle_refresh(db, panel.guild_id, Err(EditFailure::DeadTarget)).await;
    };
    let guild_id = serenity::GuildId::new(guild);

    let locale = i18n::resolve(None);
    let guild_name = guild_display_name(cache_http, guild_id, &locale).await;
    let embed =
        render::render_leaderboard(db, cache_http, guild_id, &guild_name, 1, &locale, false)
            .await?;

    let edit = serenity::ChannelId::new(channel)
        .edit_message(
            cache_http,
            serenity::MessageId::new(message),
            serenity::EditMessage::new().content("").embed(embed),
        )
        .await;

    let outcome = match edit {
        Ok(_) => Ok(()),
        Err(error) => {
            let failure = classify_error(&error);
            log::warn!(
                "Panel edit failed (guild={}, channel={channel}, message={message}, {failure:?}): {error}",
                panel.guild_id
            );
            Err(failure)
        }
    };
    settle_refresh(db, panel.guild_id, outcome).await
}

/// Debounced-path refresh: loads the guild's record (it may have been
/// deleted since the signal) and refreshes it.
async fn refresh_guild(
    db: &DatabaseConnection,
    cache_http: &impl serenity::CacheHttp,
    guild_id: i64,
) -> anyhow::Result<PanelFate> {
    match get_panel(db, guild_id).await? {
        None => Ok(PanelFate::NoPanel),
        Some(panel) => refresh_panel(db, cache_http, &panel).await,
    }
}

/// Full reconciliation sweep (F29 safety net + F32 cleanup): refreshes every
/// recorded panel with gentle pacing, aborting early on shutdown.
async fn reconcile_all(
    db: &DatabaseConnection,
    cache_http: &impl serenity::CacheHttp,
    shutdown: &watch::Receiver<bool>,
) {
    let panels = match all_panels(db).await {
        Ok(panels) => panels,
        Err(error) => {
            log::error!("Panel sweep could not list panels: {error:#}");
            return;
        }
    };
    log::info!("Panel reconciliation sweep started: {} panels", panels.len());

    let (mut updated, mut removed, mut kept) = (0usize, 0usize, 0usize);
    for panel in &panels {
        if *shutdown.borrow() {
            log::info!("Panel sweep aborted by shutdown");
            return;
        }
        match refresh_panel(db, cache_http, panel).await {
            Ok(PanelFate::Updated) => updated += 1,
            Ok(PanelFate::RemovedDead) => removed += 1,
            Ok(_) => kept += 1,
            Err(error) => {
                kept += 1;
                log::error!("Panel sweep refresh failed (guild={}): {error:#}", panel.guild_id);
            }
        }
        tokio::time::sleep(SWEEP_PACING).await;
    }
    log::info!(
        "Panel reconciliation sweep finished: {updated} updated, {removed} removed as dead, {kept} kept after errors"
    );
}

/// The background task: reacts to debounced per-guild signals (F29) and
/// runs the periodic reconciliation sweep. Exits when the shutdown watch
/// flips to `true` or either channel closes (ADR 0009).
pub async fn run(
    db: DatabaseConnection,
    cache_http: CacheAndHttp,
    mut signals: mpsc::UnboundedReceiver<i64>,
    mut shutdown: watch::Receiver<bool>,
) {
    log::info!(
        "Panel updater started (debounce {DEBOUNCE:?}, sweep every {SWEEP_INTERVAL:?})"
    );
    let mut queue = DebounceQueue::new(DEBOUNCE);
    let mut sweep = tokio::time::interval_at(
        tokio::time::Instant::now() + INITIAL_SWEEP_DELAY,
        SWEEP_INTERVAL,
    );
    sweep.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        let deadline = queue.next_deadline();
        // Placeholder far-future deadline when idle; the branch is disabled
        // via its precondition, so it never actually fires then.
        let wake = deadline
            .map(tokio::time::Instant::from_std)
            .unwrap_or_else(|| tokio::time::Instant::now() + Duration::from_secs(3600));

        tokio::select! {
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    break;
                }
            }
            received = signals.recv() => match received {
                Some(guild_id) => queue.signal(guild_id, Instant::now()),
                None => break, // every command handle dropped — shutting down
            },
            _ = sweep.tick() => reconcile_all(&db, &cache_http, &shutdown).await,
            _ = tokio::time::sleep_until(wake), if deadline.is_some() => {
                for guild_id in queue.take_due(Instant::now()) {
                    match refresh_guild(&db, &cache_http, guild_id).await {
                        Ok(fate) => log::debug!("Panel refresh for guild {guild_id}: {fate:?}"),
                        Err(error) => {
                            log::error!("Panel refresh failed (guild={guild_id}): {error:#}");
                        }
                    }
                }
            }
        }
    }
    log::info!("Panel updater stopped");
}

#[cfg(test)]
mod tests {
    use super::*;
    use sea_orm::ActiveModelTrait;

    use crate::domain::test_support::{fresh_db, insert_guild, now};

    // --- debounce queue (F29) ---

    #[test]
    fn first_signal_fixes_the_deadline_and_later_ones_coalesce() {
        let mut queue = DebounceQueue::new(Duration::from_secs(5));
        let start = Instant::now();

        queue.signal(1, start);
        let deadline = queue.next_deadline().expect("deadline set");
        assert_eq!(deadline, start + Duration::from_secs(5));

        // A later signal for the same guild must NOT push the deadline out —
        // a steady award stream still refreshes once per window.
        queue.signal(1, start + Duration::from_secs(3));
        assert_eq!(queue.next_deadline(), Some(deadline));
    }

    #[test]
    fn next_deadline_is_the_earliest_across_guilds() {
        let mut queue = DebounceQueue::new(Duration::from_secs(5));
        let start = Instant::now();
        queue.signal(1, start + Duration::from_secs(2));
        queue.signal(2, start);
        assert_eq!(queue.next_deadline(), Some(start + Duration::from_secs(5)));
    }

    #[test]
    fn take_due_returns_only_expired_guilds_and_removes_them() {
        let mut queue = DebounceQueue::new(Duration::from_secs(5));
        let start = Instant::now();
        queue.signal(1, start);
        queue.signal(2, start + Duration::from_secs(4));

        // At +5s only guild 1 is due; guild 2 stays queued.
        let due = queue.take_due(start + Duration::from_secs(5));
        assert_eq!(due, vec![1]);
        assert_eq!(queue.next_deadline(), Some(start + Duration::from_secs(9)));

        // Once taken, a guild is gone until it signals again.
        assert!(queue.take_due(start + Duration::from_secs(5)).is_empty());
        let due = queue.take_due(start + Duration::from_secs(10));
        assert_eq!(due, vec![2]);
        assert_eq!(queue.next_deadline(), None);
    }

    #[test]
    fn signal_after_take_due_schedules_a_fresh_deadline() {
        let mut queue = DebounceQueue::new(Duration::from_secs(5));
        let start = Instant::now();
        queue.signal(1, start);
        queue.take_due(start + Duration::from_secs(6));
        queue.signal(1, start + Duration::from_secs(6));
        assert_eq!(queue.next_deadline(), Some(start + Duration::from_secs(11)));
    }

    // --- error classification (F32) ---

    #[test]
    fn only_404_counts_as_a_dead_target() {
        assert!(dead_status(serenity::StatusCode::NOT_FOUND));
        for status in [
            serenity::StatusCode::FORBIDDEN,
            serenity::StatusCode::TOO_MANY_REQUESTS,
            serenity::StatusCode::INTERNAL_SERVER_ERROR,
            serenity::StatusCode::BAD_GATEWAY,
        ] {
            assert!(!dead_status(status), "{status} must be treated as transient");
        }
    }

    #[test]
    fn non_http_errors_classify_as_transient() {
        let error = serenity::Error::Other("gateway hiccup");
        assert_eq!(classify_error(&error), EditFailure::Transient);
    }

    // --- persistence (sqlite::memory:) ---

    #[tokio::test]
    async fn save_panel_inserts_and_auto_registers_the_guild_row() {
        let db = fresh_db().await;

        save_panel(&db, 1, 100, 200).await.expect("save");

        let guild = guilds::Entity::find_by_id(1).one(&db).await.expect("query");
        assert!(guild.is_some(), "guild row auto-registered for the FK");

        let panel = get_panel(&db, 1).await.expect("get").expect("row");
        assert_eq!((panel.channel_id, panel.message_id), (100, 200));
    }

    #[tokio::test]
    async fn save_panel_does_not_clobber_an_existing_guild_row() {
        let db = fresh_db().await;
        insert_guild(&db, 1).await; // is_safe = true
        save_panel(&db, 1, 100, 200).await.expect("save");
        let guild = guilds::Entity::find_by_id(1)
            .one(&db)
            .await
            .expect("query")
            .expect("row");
        assert!(guild.is_safe, "upsert must not overwrite the existing guild row");
    }

    #[tokio::test]
    async fn save_panel_replaces_the_previous_record_one_panel_per_guild() {
        let db = fresh_db().await;
        save_panel(&db, 1, 100, 200).await.expect("first save");
        save_panel(&db, 1, 111, 222).await.expect("second save");

        let panels = all_panels(&db).await.expect("list");
        assert_eq!(panels.len(), 1, "one panel per guild (PK upsert)");
        assert_eq!((panels[0].channel_id, panels[0].message_id), (111, 222));
    }

    #[tokio::test]
    async fn remove_panel_reports_whether_a_record_existed() {
        let db = fresh_db().await;
        save_panel(&db, 1, 100, 200).await.expect("save");

        assert!(remove_panel(&db, 1).await.expect("remove"), "record existed");
        assert!(!remove_panel(&db, 1).await.expect("remove again"), "already gone");
        assert!(get_panel(&db, 1).await.expect("get").is_none());
    }

    #[tokio::test]
    async fn all_panels_lists_records_in_guild_order() {
        let db = fresh_db().await;
        save_panel(&db, 7, 1, 1).await.expect("save");
        save_panel(&db, 3, 2, 2).await.expect("save");

        let panels = all_panels(&db).await.expect("list");
        let ids: Vec<i64> = panels.iter().map(|panel| panel.guild_id).collect();
        assert_eq!(ids, vec![3, 7]);
    }

    async fn insert_stale_panel(db: &DatabaseConnection, guild_id: i64) {
        insert_guild(db, guild_id).await;
        let past = now() - chrono::Duration::days(30);
        leaderboard_panels::ActiveModel {
            guild_id: Set(guild_id),
            channel_id: Set(100),
            message_id: Set(200),
            created_at: Set(past),
            updated_at: Set(past),
        }
        .insert(db)
        .await
        .expect("insert panel");
    }

    #[tokio::test]
    async fn touch_panel_bumps_updated_at_and_ignores_missing_rows() {
        let db = fresh_db().await;
        insert_stale_panel(&db, 1).await;
        let before = get_panel(&db, 1).await.expect("get").expect("row").updated_at;

        touch_panel(&db, 1).await.expect("touch");
        let after = get_panel(&db, 1).await.expect("get").expect("row").updated_at;
        assert!(after > before, "updated_at must record the successful render");

        // A guild without a panel is a no-op, not an error.
        touch_panel(&db, 999).await.expect("touch missing row");
    }

    // --- settle_refresh (F31/F32 record lifecycle) ---

    #[tokio::test]
    async fn successful_refresh_touches_the_record() {
        let db = fresh_db().await;
        insert_stale_panel(&db, 1).await;
        let before = get_panel(&db, 1).await.expect("get").expect("row").updated_at;

        let fate = settle_refresh(&db, 1, Ok(())).await.expect("settle");
        assert_eq!(fate, PanelFate::Updated);
        let after = get_panel(&db, 1).await.expect("get").expect("row").updated_at;
        assert!(after > before);
    }

    #[tokio::test]
    async fn dead_target_removes_the_record_f32() {
        let db = fresh_db().await;
        insert_stale_panel(&db, 1).await;

        let fate = settle_refresh(&db, 1, Err(EditFailure::DeadTarget))
            .await
            .expect("settle");
        assert_eq!(fate, PanelFate::RemovedDead);
        assert!(
            get_panel(&db, 1).await.expect("get").is_none(),
            "stale record must be swept away"
        );
    }

    #[tokio::test]
    async fn transient_failure_keeps_the_record_untouched() {
        let db = fresh_db().await;
        insert_stale_panel(&db, 1).await;
        let before = get_panel(&db, 1).await.expect("get").expect("row").updated_at;

        let fate = settle_refresh(&db, 1, Err(EditFailure::Transient))
            .await
            .expect("settle");
        assert_eq!(fate, PanelFate::KeptAfterError);
        let row = get_panel(&db, 1).await.expect("get").expect("row");
        assert_eq!(row.updated_at, before, "no touch on failure — retried next time");
    }

    // --- signal handle ---

    #[tokio::test]
    async fn notify_delivers_guild_ids_and_survives_a_closed_channel() {
        let (signal, mut rx) = signal_channel();
        signal.notify(42);
        assert_eq!(rx.recv().await, Some(42));

        drop(rx);
        signal.notify(43); // must not panic — only logs
    }
}
