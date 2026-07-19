//! Background per-category "active medals" catalog panel updater and its
//! persistence layer (`active_medals_panels`).
//!
//! Deliberately a SEPARATE module/table from [`crate::bot::panel_updater`]:
//! that one is hard-wired to exactly one score-driven leaderboard panel per
//! guild (PK on `guild_id`); this one allows any number of panels per guild,
//! one per `category`, driven by trophy create/edit/delete instead of
//! award/revoke/clear. The overall shape — persistence helpers, debounced
//! signal, low-frequency reconciliation sweep, F31/F32-style record
//! lifecycle — mirrors `panel_updater.rs`; shared pieces that don't depend on
//! the "one row per guild" assumption ([`crate::bot::panel_updater::CacheAndHttp`],
//! [`crate::bot::panel_updater::SweepSlot`], error classification, debounce
//! constants) are reused directly rather than duplicated.
//!
//! Rendered content: every ACTIVE trophy in the guild+category, name and
//! description, no score data — a live catalog, not a leaderboard. Panels
//! render with the default locale (`i18n::resolve(None)`), matching the
//! leaderboard panel's rationale: no interaction locale to inherit, and the
//! message must not flip language between refreshes.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use poise::serenity_prelude as serenity;
use sea_orm::sea_query::OnConflict;
use sea_orm::{
    ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder, QuerySelect, Set,
    TransactionTrait,
};
use tokio::sync::{mpsc, watch};
use uuid::Uuid;

use crate::bot::panel_updater::{classify_error, CacheAndHttp, EditFailure, SweepSlot, DEBOUNCE};
use crate::entities::{active_medals_panels, guilds, medals_overview_panels, trophies};
use crate::i18n::{self, LanguageIdentifier};

/// Low-frequency reconciliation sweep interval — same cadence as the
/// leaderboard panel updater.
pub(crate) const SWEEP_INTERVAL: Duration = Duration::from_secs(15 * 60);

/// Delay before the FIRST sweep after startup.
const INITIAL_SWEEP_DELAY: Duration = Duration::from_secs(60);

/// Pause between panels inside a sweep (rate-limit friendliness).
const SWEEP_PACING: Duration = Duration::from_secs(1);

// ---------------------------------------------------------------------------
// Signal handle (lives on `Data`)
// ---------------------------------------------------------------------------

/// A guild + category pair — the key every panel and every signal is scoped to.
pub type PanelKey = (i64, String);

/// Cheap cloneable handle used by trophy-editing commands to request a
/// debounced refresh of one category's panel.
#[derive(Clone)]
pub struct PanelSignal(mpsc::UnboundedSender<PanelKey>);

impl PanelSignal {
    /// Fire-and-forget: never blocks a command handler.
    pub fn notify(&self, guild_id: i64, category: impl Into<String>) {
        let key = (guild_id, category.into());
        if self.0.send(key.clone()).is_err() {
            log::warn!("Medals panel updater not running; dropped refresh signal for {key:?}");
        }
    }
}

/// Creates the signal channel pair: the sender goes into `Data`, the
/// receiver into [`run`].
pub fn signal_channel() -> (PanelSignal, mpsc::UnboundedReceiver<PanelKey>) {
    let (tx, rx) = mpsc::unbounded_channel();
    (PanelSignal(tx), rx)
}

// ---------------------------------------------------------------------------
// Debounce queue (pure, testable) — same rules as panel_updater's, keyed by
// (guild_id, category) instead of guild_id alone.
// ---------------------------------------------------------------------------

pub(crate) struct DebounceQueue {
    delay: Duration,
    pending: HashMap<PanelKey, Instant>,
}

impl DebounceQueue {
    pub(crate) fn new(delay: Duration) -> Self {
        Self { delay, pending: HashMap::new() }
    }

    pub(crate) fn signal(&mut self, key: PanelKey, now: Instant) {
        self.pending.entry(key).or_insert(now + self.delay);
    }

    pub(crate) fn next_deadline(&self) -> Option<Instant> {
        self.pending.values().min().copied()
    }

    pub(crate) fn take_one_due(&mut self, now: Instant) -> Option<PanelKey> {
        let key = self
            .pending
            .iter()
            .filter(|(_, deadline)| **deadline <= now)
            .min_by_key(|(_, deadline)| **deadline)
            .map(|(key, _)| key.clone())?;
        self.pending.remove(&key);
        Some(key)
    }
}

// ---------------------------------------------------------------------------
// Persistence helpers (active_medals_panels table)
// ---------------------------------------------------------------------------

/// The panel record for `(guild_id, category)`, if one exists.
pub(crate) async fn get_panel(
    db: &DatabaseConnection,
    guild_id: i64,
    category: &str,
) -> anyhow::Result<Option<active_medals_panels::Model>> {
    Ok(active_medals_panels::Entity::find()
        .filter(active_medals_panels::Column::GuildId.eq(guild_id))
        .filter(active_medals_panels::Column::Category.eq(category))
        .one(db)
        .await?)
}

/// Every panel record, in stable id order (sweep input).
pub(crate) async fn all_panels(
    db: &DatabaseConnection,
) -> anyhow::Result<Vec<active_medals_panels::Model>> {
    Ok(active_medals_panels::Entity::find()
        .order_by_asc(active_medals_panels::Column::Id)
        .all(db)
        .await?)
}

/// The guild's distinct categorized-trophy categories, alphabetical — used
/// to autocomplete `/panel medals create`'s `category` option.
pub async fn distinct_categories(
    db: &DatabaseConnection,
    guild_id: i64,
) -> anyhow::Result<Vec<String>> {
    let categories: Vec<Option<String>> = trophies::Entity::find()
        .filter(trophies::Column::GuildId.eq(guild_id))
        .filter(trophies::Column::Category.is_not_null())
        .select_only()
        .column(trophies::Column::Category)
        .distinct()
        .order_by_asc(trophies::Column::Category)
        .into_tuple()
        .all(db)
        .await?;
    Ok(categories.into_iter().flatten().collect())
}

/// Records the panel message for `(guild_id, category)`, replacing any
/// previous record for that same pair (upsert on the unique index).
/// Auto-registers the guild row so the FK holds without clobbering an
/// existing row. F31: callers MUST invoke this only after the Discord
/// message was successfully sent.
pub(crate) async fn save_panel(
    db: &DatabaseConnection,
    guild_id: i64,
    category: &str,
    channel_id: i64,
    message_id: i64,
    source_guild_id: Option<i64>,
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

    active_medals_panels::Entity::insert(active_medals_panels::ActiveModel {
        id: Set(Uuid::now_v7()),
        guild_id: Set(guild_id),
        category: Set(category.to_string()),
        channel_id: Set(channel_id),
        message_id: Set(message_id),
        source_guild_id: Set(source_guild_id),
        created_at: Set(now),
        updated_at: Set(now),
    })
    .on_conflict(
        OnConflict::columns([
            active_medals_panels::Column::GuildId,
            active_medals_panels::Column::Category,
        ])
        .update_columns([
            active_medals_panels::Column::ChannelId,
            active_medals_panels::Column::MessageId,
            active_medals_panels::Column::SourceGuildId,
            active_medals_panels::Column::UpdatedAt,
        ])
        .to_owned(),
    )
    .exec_without_returning(&txn)
    .await?;

    txn.commit().await?;
    Ok(())
}

/// Deletes the panel record for `(guild_id, category)`. Returns whether one
/// existed.
pub(crate) async fn remove_panel(
    db: &DatabaseConnection,
    guild_id: i64,
    category: &str,
) -> anyhow::Result<bool> {
    let result = active_medals_panels::Entity::delete_many()
        .filter(active_medals_panels::Column::GuildId.eq(guild_id))
        .filter(active_medals_panels::Column::Category.eq(category))
        .exec(db)
        .await?;
    Ok(result.rows_affected > 0)
}

/// Every category panel `guild_id` has pointed at `source_guild_id` — used
/// by `/link revoke` to find every panel that must be torn down when a link
/// is severed (a linked guild may have panels for several categories).
pub(crate) async fn panels_from_source(
    db: &DatabaseConnection,
    guild_id: i64,
    source_guild_id: i64,
) -> anyhow::Result<Vec<active_medals_panels::Model>> {
    Ok(active_medals_panels::Entity::find()
        .filter(active_medals_panels::Column::GuildId.eq(guild_id))
        .filter(active_medals_panels::Column::SourceGuildId.eq(source_guild_id))
        .all(db)
        .await?)
}

/// Deletes ONE panel record by primary key, only if it still points at
/// `message_id` (F32 stale-refresh guard — a concurrent `/panel medals
/// create` may have replaced the record while a refresh of the OLD message
/// was in flight).
pub(crate) async fn remove_panel_if_message(
    db: &DatabaseConnection,
    id: Uuid,
    message_id: i64,
) -> anyhow::Result<bool> {
    let result = active_medals_panels::Entity::delete_many()
        .filter(active_medals_panels::Column::Id.eq(id))
        .filter(active_medals_panels::Column::MessageId.eq(message_id))
        .exec(db)
        .await?;
    Ok(result.rows_affected > 0)
}

/// Marks a successful render (`updated_at` doubles as "last successful
/// render"). A vanished row is a no-op, not an error.
pub(crate) async fn touch_panel(db: &DatabaseConnection, id: Uuid) -> anyhow::Result<()> {
    active_medals_panels::Entity::update_many()
        .set(active_medals_panels::ActiveModel {
            updated_at: Set(chrono::Utc::now().naive_utc()),
            ..Default::default()
        })
        .filter(active_medals_panels::Column::Id.eq(id))
        .exec(db)
        .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Overview panel persistence (medals_overview_panels — one per guild, PK)
// ---------------------------------------------------------------------------

/// The overview panel record for `guild_id`, if one exists (one per guild — PK).
pub(crate) async fn get_overview_panel(
    db: &DatabaseConnection,
    guild_id: i64,
) -> anyhow::Result<Option<medals_overview_panels::Model>> {
    Ok(medals_overview_panels::Entity::find_by_id(guild_id).one(db).await?)
}

/// Every overview panel record, in stable guild order (sweep input).
pub(crate) async fn all_overview_panels(
    db: &DatabaseConnection,
) -> anyhow::Result<Vec<medals_overview_panels::Model>> {
    Ok(medals_overview_panels::Entity::find()
        .order_by_asc(medals_overview_panels::Column::GuildId)
        .all(db)
        .await?)
}

/// Every overview panel whose EFFECTIVE data source is `guild_id` — its own
/// panel (if any) plus every linked guild's overview panel mirroring it.
pub(crate) async fn overview_panels_for_effective_guild(
    db: &DatabaseConnection,
    guild_id: i64,
) -> anyhow::Result<Vec<medals_overview_panels::Model>> {
    Ok(medals_overview_panels::Entity::find()
        .filter(
            sea_orm::Condition::any()
                .add(medals_overview_panels::Column::GuildId.eq(guild_id))
                .add(medals_overview_panels::Column::SourceGuildId.eq(guild_id)),
        )
        .all(db)
        .await?)
}

/// Records the overview panel message for a guild, replacing any previous
/// record (one panel per guild — PK upsert). Auto-registers the guild row.
/// F31: callers MUST invoke this only after the Discord message was
/// successfully sent.
pub(crate) async fn save_overview_panel(
    db: &DatabaseConnection,
    guild_id: i64,
    channel_id: i64,
    message_id: i64,
    source_guild_id: Option<i64>,
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

    medals_overview_panels::Entity::insert(medals_overview_panels::ActiveModel {
        guild_id: Set(guild_id),
        channel_id: Set(channel_id),
        message_id: Set(message_id),
        source_guild_id: Set(source_guild_id),
        created_at: Set(now),
        updated_at: Set(now),
    })
    .on_conflict(
        OnConflict::column(medals_overview_panels::Column::GuildId)
            .update_columns([
                medals_overview_panels::Column::ChannelId,
                medals_overview_panels::Column::MessageId,
                medals_overview_panels::Column::SourceGuildId,
                medals_overview_panels::Column::UpdatedAt,
            ])
            .to_owned(),
    )
    .exec_without_returning(&txn)
    .await?;

    txn.commit().await?;
    Ok(())
}

/// Deletes the guild's overview panel record. Returns whether one existed.
pub(crate) async fn remove_overview_panel(db: &DatabaseConnection, guild_id: i64) -> anyhow::Result<bool> {
    let result = medals_overview_panels::Entity::delete_by_id(guild_id).exec(db).await?;
    Ok(result.rows_affected > 0)
}

/// Deletes the guild's overview panel record ONLY if it still points at
/// `message_id` (F32 stale-refresh guard, same rationale as `panel_updater`).
pub(crate) async fn remove_overview_panel_if_message(
    db: &DatabaseConnection,
    guild_id: i64,
    message_id: i64,
) -> anyhow::Result<bool> {
    let result = medals_overview_panels::Entity::delete_many()
        .filter(medals_overview_panels::Column::GuildId.eq(guild_id))
        .filter(medals_overview_panels::Column::MessageId.eq(message_id))
        .exec(db)
        .await?;
    Ok(result.rows_affected > 0)
}

/// Marks a successful render. A vanished row is a no-op, not an error.
pub(crate) async fn touch_overview_panel(db: &DatabaseConnection, guild_id: i64) -> anyhow::Result<()> {
    medals_overview_panels::Entity::update_many()
        .set(medals_overview_panels::ActiveModel {
            updated_at: Set(chrono::Utc::now().naive_utc()),
            ..Default::default()
        })
        .filter(medals_overview_panels::Column::GuildId.eq(guild_id))
        .exec(db)
        .await?;
    Ok(())
}

/// Best-effort deletion of an overview panel's Discord message.
pub(crate) async fn delete_overview_panel_message(
    http: impl AsRef<serenity::Http>,
    panel: &medals_overview_panels::Model,
) {
    let (Ok(channel_id), Ok(message_id)) =
        (u64::try_from(panel.channel_id), u64::try_from(panel.message_id))
    else {
        return;
    };
    if let Err(error) = serenity::ChannelId::new(channel_id)
        .delete_message(http, serenity::MessageId::new(message_id))
        .await
    {
        log::warn!(
            "Could not delete medals overview panel message (guild={}, channel={channel_id}, message={message_id}): {error}",
            panel.guild_id
        );
    }
}

/// Poise autocomplete callback for `/panel medals`'s `category` option: the
/// guild's distinct categories, prefix-matched case-insensitively.
pub async fn autocomplete_category(ctx: crate::bot::Context<'_>, partial: &str) -> Vec<String> {
    let Some(guild_id) = ctx.guild_id() else {
        return Vec::new();
    };
    let db = &ctx.data().db;
    // Cross-guild link: a linked guild has no categories of its own — show
    // the source guild's instead, matching what `/panel medals create` will
    // actually render.
    let effective = crate::domain::guild_links::accepted_source_for(db, guild_id.get() as i64)
        .await
        .unwrap_or_default()
        .unwrap_or(guild_id.get() as i64);
    match distinct_categories(db, effective).await {
        Ok(categories) => {
            let needle = partial.to_lowercase();
            categories
                .into_iter()
                .filter(|c| c.to_lowercase().starts_with(&needle))
                .take(25)
                .collect()
        }
        Err(err) => {
            log::warn!("category autocomplete query failed (guild={}): {err}", guild_id.get());
            Vec::new()
        }
    }
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

/// One `emoji **name** — description` line, shared by the single-category
/// and overview renderers.
fn medal_row(locale: &LanguageIdentifier, medal: &trophies::Model) -> String {
    i18n::t_args(
        locale,
        "medals-panel-row",
        &[
            ("emoji", medal.emoji.clone().into()),
            ("name", medal.name.clone().into()),
            ("description", medal.description.clone().into()),
        ],
    )
}

/// Builds the catalog embed for one guild+category: every ACTIVE trophy in
/// it, alphabetical, name + description. Empty → a localized placeholder.
pub async fn render_category_embed(
    db: &DatabaseConnection,
    guild_id: i64,
    category: &str,
    locale: &LanguageIdentifier,
) -> anyhow::Result<serenity::CreateEmbed> {
    let medals = trophies::Entity::find()
        .filter(trophies::Column::GuildId.eq(guild_id))
        .filter(trophies::Column::Category.eq(category))
        .filter(trophies::Column::Active.eq(true))
        .order_by_asc(trophies::Column::Name)
        .all(db)
        .await?;

    let body = if medals.is_empty() {
        i18n::t(locale, "medals-panel-empty")
    } else {
        medals.iter().map(|t| medal_row(locale, t)).collect::<Vec<_>>().join("\n")
    };

    Ok(serenity::CreateEmbed::new()
        .title(i18n::t_args(locale, "medals-panel-title", &[("category", category.to_string().into())]))
        .description(body)
        .colour(crate::bot::util::COLOR_MAIN))
}

/// Discord's hard cap on embed fields — categories beyond this are dropped
/// (alphabetically last), with a footer note.
const MAX_OVERVIEW_FIELDS: usize = 25;
/// Discord's hard cap on one embed field's value.
const MAX_FIELD_VALUE_CHARS: usize = 1024;

/// Builds the "all categories" catalog embed: every ACTIVE, CATEGORIZED
/// trophy in the guild, one field per category (alphabetical), each field
/// listing that category's medals the same way [`render_category_embed`]
/// does. Uncategorized trophies never appear here either — consistent with
/// the single-category panel. Empty → a localized placeholder.
pub async fn render_overview_embed(
    db: &DatabaseConnection,
    guild_id: i64,
    locale: &LanguageIdentifier,
) -> anyhow::Result<serenity::CreateEmbed> {
    let medals = trophies::Entity::find()
        .filter(trophies::Column::GuildId.eq(guild_id))
        .filter(trophies::Column::Category.is_not_null())
        .filter(trophies::Column::Active.eq(true))
        .order_by_asc(trophies::Column::Category)
        .order_by_asc(trophies::Column::Name)
        .all(db)
        .await?;

    let mut embed = serenity::CreateEmbed::new()
        .title(i18n::t(locale, "medals-overview-title"))
        .colour(crate::bot::util::COLOR_MAIN);

    if medals.is_empty() {
        return Ok(embed.description(i18n::t(locale, "medals-overview-empty")));
    }

    // `medals` is already sorted by category then name, so same-category
    // trophies are contiguous — one pass groups them without a HashMap.
    let mut categories: Vec<(String, Vec<&trophies::Model>)> = Vec::new();
    for medal in &medals {
        let category = medal.category.clone().expect("filtered to categorized trophies");
        match categories.last_mut() {
            Some((name, group)) if *name == category => group.push(medal),
            _ => categories.push((category, vec![medal])),
        }
    }

    let truncated = categories.len() > MAX_OVERVIEW_FIELDS;
    for (category, group) in categories.into_iter().take(MAX_OVERVIEW_FIELDS) {
        let mut value = group.iter().map(|t| medal_row(locale, t)).collect::<Vec<_>>().join("\n");
        if value.chars().count() > MAX_FIELD_VALUE_CHARS {
            let note = i18n::t(locale, "medals-overview-truncated");
            let budget = MAX_FIELD_VALUE_CHARS.saturating_sub(note.chars().count() + 1);
            let cut: String = value.chars().take(budget).collect();
            value = format!("{cut}\n{note}");
        }
        embed = embed.field(category, value, false);
    }
    if truncated {
        embed = embed.footer(serenity::CreateEmbedFooter::new(i18n::t(
            locale,
            "medals-overview-more-categories",
        )));
    }
    Ok(embed)
}

// ---------------------------------------------------------------------------
// Refresh outcome handling (F32) — same shape as panel_updater's.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PanelFate {
    Updated,
    RemovedDead,
    Superseded,
    KeptAfterError,
}

pub(crate) async fn settle_refresh(
    db: &DatabaseConnection,
    panel: &active_medals_panels::Model,
    outcome: Result<(), EditFailure>,
) -> anyhow::Result<PanelFate> {
    match outcome {
        Ok(()) => {
            touch_panel(db, panel.id).await?;
            Ok(PanelFate::Updated)
        }
        Err(EditFailure::DeadTarget) => {
            if remove_panel_if_message(db, panel.id, panel.message_id).await? {
                log::info!(
                    "Removed medals panel {} (guild={}, category={:?}): target channel/message is gone (F32)",
                    panel.id,
                    panel.guild_id,
                    panel.category
                );
                Ok(PanelFate::RemovedDead)
            } else {
                Ok(PanelFate::Superseded)
            }
        }
        Err(EditFailure::Transient) => Ok(PanelFate::KeptAfterError),
    }
}

/// Best-effort deletion of a panel's Discord message.
pub(crate) async fn delete_panel_message(
    http: impl AsRef<serenity::Http>,
    panel: &active_medals_panels::Model,
) {
    let (Ok(channel_id), Ok(message_id)) =
        (u64::try_from(panel.channel_id), u64::try_from(panel.message_id))
    else {
        return;
    };
    if let Err(error) = serenity::ChannelId::new(channel_id)
        .delete_message(http, serenity::MessageId::new(message_id))
        .await
    {
        log::warn!(
            "Could not delete medals panel message (guild={}, category={:?}, channel={channel_id}, message={message_id}): {error}",
            panel.guild_id,
            panel.category
        );
    }
}

/// Re-renders one panel record and edits its message, then settles the
/// record per the outcome.
pub(crate) async fn refresh_panel(
    db: &DatabaseConnection,
    cache_http: &impl serenity::CacheHttp,
    panel: &active_medals_panels::Model,
) -> anyhow::Result<PanelFate> {
    let (Ok(channel), Ok(message)) =
        (u64::try_from(panel.channel_id), u64::try_from(panel.message_id))
    else {
        return settle_refresh(db, panel, Err(EditFailure::DeadTarget)).await;
    };

    // Cross-guild link: render the source guild's category instead of this
    // panel's own, re-validating the link is still accepted on EVERY
    // refresh (not just trusting the stored column) — a revoked link stops
    // leaking data on the very next refresh.
    let data_guild_id = match panel.source_guild_id {
        Some(source) => {
            if !crate::domain::guild_links::is_accepted(db, source, panel.guild_id).await? {
                log::info!(
                    "Removing medals panel {} (guild={}, category={:?}): link to source guild {source} is no longer accepted",
                    panel.id,
                    panel.guild_id,
                    panel.category
                );
                return settle_refresh(db, panel, Err(EditFailure::DeadTarget)).await;
            }
            source
        }
        None => panel.guild_id,
    };

    let locale = i18n::resolve(None);
    let embed = render_category_embed(db, data_guild_id, &panel.category, &locale).await?;

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
                "Medals panel edit failed (guild={}, category={:?}, channel={channel}, message={message}, {failure:?}): {error}",
                panel.guild_id,
                panel.category
            );
            Err(failure)
        }
    };
    settle_refresh(db, panel, outcome).await
}

/// [`settle_refresh`]'s counterpart for the guild-PK-keyed overview panel.
pub(crate) async fn settle_refresh_overview(
    db: &DatabaseConnection,
    panel: &medals_overview_panels::Model,
    outcome: Result<(), EditFailure>,
) -> anyhow::Result<PanelFate> {
    match outcome {
        Ok(()) => {
            touch_overview_panel(db, panel.guild_id).await?;
            Ok(PanelFate::Updated)
        }
        Err(EditFailure::DeadTarget) => {
            if remove_overview_panel_if_message(db, panel.guild_id, panel.message_id).await? {
                log::info!(
                    "Removed medals overview panel for guild {}: target channel/message is gone (F32)",
                    panel.guild_id
                );
                Ok(PanelFate::RemovedDead)
            } else {
                Ok(PanelFate::Superseded)
            }
        }
        Err(EditFailure::Transient) => Ok(PanelFate::KeptAfterError),
    }
}

/// [`refresh_panel`]'s counterpart for the overview panel.
pub(crate) async fn refresh_overview_panel(
    db: &DatabaseConnection,
    cache_http: &impl serenity::CacheHttp,
    panel: &medals_overview_panels::Model,
) -> anyhow::Result<PanelFate> {
    let (Ok(channel), Ok(message)) =
        (u64::try_from(panel.channel_id), u64::try_from(panel.message_id))
    else {
        return settle_refresh_overview(db, panel, Err(EditFailure::DeadTarget)).await;
    };

    let data_guild_id = match panel.source_guild_id {
        Some(source) => {
            if !crate::domain::guild_links::is_accepted(db, source, panel.guild_id).await? {
                log::info!(
                    "Removing medals overview panel (guild={}): link to source guild {source} is no longer accepted",
                    panel.guild_id
                );
                return settle_refresh_overview(db, panel, Err(EditFailure::DeadTarget)).await;
            }
            source
        }
        None => panel.guild_id,
    };

    let locale = i18n::resolve(None);
    let embed = render_overview_embed(db, data_guild_id, &locale).await?;

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
                "Medals overview panel edit failed (guild={}, channel={channel}, message={message}, {failure:?}): {error}",
                panel.guild_id
            );
            Err(failure)
        }
    };
    settle_refresh_overview(db, panel, outcome).await
}

/// Every panel whose EFFECTIVE data source is `(guild_id, category)` — its
/// own panel (if any) plus every linked guild's panel mirroring it.
pub(crate) async fn panels_for_effective_key(
    db: &DatabaseConnection,
    guild_id: i64,
    category: &str,
) -> anyhow::Result<Vec<active_medals_panels::Model>> {
    Ok(active_medals_panels::Entity::find()
        .filter(active_medals_panels::Column::Category.eq(category))
        .filter(
            sea_orm::Condition::any()
                .add(active_medals_panels::Column::GuildId.eq(guild_id))
                .add(active_medals_panels::Column::SourceGuildId.eq(guild_id)),
        )
        .all(db)
        .await?)
}

/// Refreshes every category panel for `key`, plus the overview panel(s) for
/// its guild — an overview panel piggybacks on the SAME per-category signal
/// (any category change is exactly what the overview must reflect too),
/// rather than needing its own signal channel/debounce queue.
async fn refresh_key(
    db: &DatabaseConnection,
    cache_http: &impl serenity::CacheHttp,
    key: &PanelKey,
) -> anyhow::Result<Vec<PanelFate>> {
    let (guild_id, category) = key;
    let panels = panels_for_effective_key(db, *guild_id, category).await?;
    let mut fates = Vec::with_capacity(panels.len());
    for panel in &panels {
        fates.push(refresh_panel(db, cache_http, panel).await?);
    }
    for panel in overview_panels_for_effective_guild(db, *guild_id).await? {
        fates.push(refresh_overview_panel(db, cache_http, &panel).await?);
    }
    Ok(fates)
}

async fn reconcile_all(
    db: &DatabaseConnection,
    cache_http: &impl serenity::CacheHttp,
    shutdown: &watch::Receiver<bool>,
) {
    let panels = match all_panels(db).await {
        Ok(panels) => panels,
        Err(error) => {
            log::error!("Medals panel sweep could not list panels: {error:#}");
            return;
        }
    };
    let overview_panels = match all_overview_panels(db).await {
        Ok(panels) => panels,
        Err(error) => {
            log::error!("Medals overview panel sweep could not list panels: {error:#}");
            Vec::new()
        }
    };
    log::info!(
        "Medals panel reconciliation sweep started: {} category panels, {} overview panels",
        panels.len(),
        overview_panels.len()
    );

    let (mut updated, mut removed, mut kept) = (0usize, 0usize, 0usize);
    for panel in &panels {
        if *shutdown.borrow() {
            log::info!("Medals panel sweep aborted by shutdown");
            return;
        }
        match refresh_panel(db, cache_http, panel).await {
            Ok(PanelFate::Updated) => updated += 1,
            Ok(PanelFate::RemovedDead) => removed += 1,
            Ok(_) => kept += 1,
            Err(error) => {
                kept += 1;
                log::error!("Medals panel sweep refresh failed (id={}): {error:#}", panel.id);
            }
        }
        tokio::time::sleep(SWEEP_PACING).await;
    }
    for panel in &overview_panels {
        if *shutdown.borrow() {
            log::info!("Medals overview panel sweep aborted by shutdown");
            return;
        }
        match refresh_overview_panel(db, cache_http, panel).await {
            Ok(PanelFate::Updated) => updated += 1,
            Ok(PanelFate::RemovedDead) => removed += 1,
            Ok(_) => kept += 1,
            Err(error) => {
                kept += 1;
                log::error!(
                    "Medals overview panel sweep refresh failed (guild={}): {error:#}",
                    panel.guild_id
                );
            }
        }
        tokio::time::sleep(SWEEP_PACING).await;
    }
    log::info!(
        "Medals panel reconciliation sweep finished: {updated} updated, {removed} removed as dead, {kept} kept after errors"
    );
}

/// The background task: reacts to debounced per-category signals and runs
/// the periodic reconciliation sweep. Exits when the shutdown watch flips to
/// `true` or either channel closes (ADR 0009).
pub async fn run(
    db: DatabaseConnection,
    cache_http: CacheAndHttp,
    mut signals: mpsc::UnboundedReceiver<PanelKey>,
    mut shutdown: watch::Receiver<bool>,
) {
    log::info!(
        "Medals panel updater started (debounce {DEBOUNCE:?}, sweep every {SWEEP_INTERVAL:?})"
    );
    let mut queue = DebounceQueue::new(DEBOUNCE);
    let mut sweep_slot = SweepSlot::new();
    let mut sweep = tokio::time::interval_at(
        tokio::time::Instant::now() + INITIAL_SWEEP_DELAY,
        SWEEP_INTERVAL,
    );
    sweep.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        let deadline = queue.next_deadline();
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
                Some(key) => queue.signal(key, Instant::now()),
                None => break,
            },
            _ = sweep.tick() => {
                let started = sweep_slot.try_start({
                    let db = db.clone();
                    let cache_http = cache_http.clone();
                    let shutdown = shutdown.clone();
                    async move { reconcile_all(&db, &cache_http, &shutdown).await }
                });
                if !started {
                    log::warn!("Medals panel sweep tick skipped: previous sweep still running");
                }
            }
            _ = tokio::time::sleep_until(wake), if deadline.is_some() => {
                if let Some(key) = queue.take_one_due(Instant::now()) {
                    match refresh_key(&db, &cache_http, &key).await {
                        Ok(fates) => log::debug!("Medals panel refresh for {key:?}: {fates:?}"),
                        Err(error) => {
                            log::error!("Medals panel refresh failed ({key:?}): {error:#}");
                        }
                    }
                }
            }
        }
    }
    sweep_slot.shutdown().await;
    log::info!("Medals panel updater stopped");
}

#[cfg(test)]
mod tests {
    use super::*;
    use sea_orm::ActiveModelTrait;

    use crate::domain::test_support::{fresh_db, insert_guild, now};

    // --- debounce queue ---

    #[test]
    fn first_signal_fixes_the_deadline_and_later_ones_coalesce() {
        let mut queue = DebounceQueue::new(Duration::from_secs(5));
        let start = Instant::now();
        let key = (1, "Government".to_string());

        queue.signal(key.clone(), start);
        let deadline = queue.next_deadline().expect("deadline set");
        assert_eq!(deadline, start + Duration::from_secs(5));

        queue.signal(key, start + Duration::from_secs(3));
        assert_eq!(queue.next_deadline(), Some(deadline));
    }

    #[test]
    fn different_categories_of_the_same_guild_are_independent_keys() {
        let mut queue = DebounceQueue::new(Duration::from_secs(5));
        let start = Instant::now();
        queue.signal((1, "Government".to_string()), start);
        queue.signal((1, "Recurring".to_string()), start);

        let mut due = Vec::new();
        while let Some(key) = queue.take_one_due(start + Duration::from_secs(5)) {
            due.push(key);
        }
        due.sort();
        assert_eq!(
            due,
            vec![(1, "Government".to_string()), (1, "Recurring".to_string())]
        );
    }

    // --- persistence (sqlite::memory:) ---

    #[tokio::test]
    async fn save_panel_inserts_and_auto_registers_the_guild_row() {
        let db = fresh_db().await;

        save_panel(&db, 1, "Government", 100, 200, None).await.expect("save");

        let guild = guilds::Entity::find_by_id(1).one(&db).await.expect("query");
        assert!(guild.is_some(), "guild row auto-registered for the FK");

        let panel = get_panel(&db, 1, "Government").await.expect("get").expect("row");
        assert_eq!((panel.channel_id, panel.message_id), (100, 200));
    }

    #[tokio::test]
    async fn save_panel_upserts_per_category_not_per_guild() {
        let db = fresh_db().await;
        save_panel(&db, 1, "Government", 100, 200, None).await.expect("save gov");
        save_panel(&db, 1, "Recurring", 111, 222, None).await.expect("save rec");
        save_panel(&db, 1, "Government", 100, 999, None).await.expect("replace gov");

        let panels = all_panels(&db).await.expect("list");
        assert_eq!(panels.len(), 2, "one panel per category, not per guild");
        let gov = get_panel(&db, 1, "Government").await.expect("get").expect("row");
        assert_eq!(gov.message_id, 999, "same-category save replaces in place");
        let rec = get_panel(&db, 1, "Recurring").await.expect("get").expect("row");
        assert_eq!(rec.message_id, 222, "other categories are untouched");
    }

    #[tokio::test]
    async fn remove_panel_reports_whether_a_record_existed() {
        let db = fresh_db().await;
        save_panel(&db, 1, "Government", 100, 200, None).await.expect("save");

        assert!(remove_panel(&db, 1, "Government").await.expect("remove"));
        assert!(!remove_panel(&db, 1, "Government").await.expect("remove again"));
        assert!(get_panel(&db, 1, "Government").await.expect("get").is_none());
    }

    #[tokio::test]
    async fn remove_panel_if_message_only_deletes_the_matching_record() {
        let db = fresh_db().await;
        save_panel(&db, 1, "Government", 100, 200, None).await.expect("save");
        let panel = get_panel(&db, 1, "Government").await.expect("get").expect("row");

        assert!(!remove_panel_if_message(&db, panel.id, 999).await.expect("mismatch"));
        assert!(get_panel(&db, 1, "Government").await.expect("get").is_some());

        assert!(remove_panel_if_message(&db, panel.id, 200).await.expect("match"));
        assert!(get_panel(&db, 1, "Government").await.expect("get").is_none());
    }

    #[tokio::test]
    async fn touch_panel_bumps_updated_at_and_ignores_missing_rows() {
        let db = fresh_db().await;
        save_panel(&db, 1, "Government", 100, 200, None).await.expect("save");
        let before = get_panel(&db, 1, "Government").await.expect("get").expect("row").updated_at;

        let id = get_panel(&db, 1, "Government").await.expect("get").expect("row").id;
        touch_panel(&db, id).await.expect("touch");
        let after = get_panel(&db, 1, "Government").await.expect("get").expect("row").updated_at;
        assert!(after >= before);

        touch_panel(&db, Uuid::now_v7()).await.expect("touch missing row");
    }

    // --- settle_refresh (F31/F32 record lifecycle) ---

    // --- panels_for_effective_key (cross-guild link fan-out) ---

    #[tokio::test]
    async fn panels_for_effective_key_includes_own_panel_and_every_linked_one() {
        let db = fresh_db().await;
        save_panel(&db, 1, "Government", 100, 200, None).await.expect("save A");
        save_panel(&db, 2, "Government", 300, 400, Some(1)).await.expect("save B");
        save_panel(&db, 3, "Government", 500, 600, Some(1)).await.expect("save C");
        // Different category — must not match even for the same source.
        save_panel(&db, 1, "Recurring", 700, 800, None).await.expect("save A recurring");
        // Different source guild — must not match A's signal.
        save_panel(&db, 4, "Government", 900, 1000, Some(9)).await.expect("save D");

        let mut guild_ids: Vec<i64> = panels_for_effective_key(&db, 1, "Government")
            .await
            .unwrap()
            .iter()
            .map(|p| p.guild_id)
            .collect();
        guild_ids.sort();
        assert_eq!(guild_ids, vec![1, 2, 3]);
    }

    #[tokio::test]
    async fn panels_for_effective_key_is_empty_when_nothing_matches() {
        let db = fresh_db().await;
        save_panel(&db, 1, "Government", 100, 200, None).await.expect("save");
        assert!(panels_for_effective_key(&db, 1, "Recurring").await.unwrap().is_empty());
        assert!(panels_for_effective_key(&db, 999, "Government").await.unwrap().is_empty());
    }

    // --- overview panel persistence ---

    #[tokio::test]
    async fn save_overview_panel_inserts_and_auto_registers_the_guild_row() {
        let db = fresh_db().await;
        save_overview_panel(&db, 1, 100, 200, None).await.expect("save");

        let guild = guilds::Entity::find_by_id(1).one(&db).await.expect("query");
        assert!(guild.is_some(), "guild row auto-registered for the FK");

        let panel = get_overview_panel(&db, 1).await.expect("get").expect("row");
        assert_eq!((panel.channel_id, panel.message_id), (100, 200));
    }

    #[tokio::test]
    async fn save_overview_panel_replaces_the_previous_record_one_per_guild() {
        let db = fresh_db().await;
        save_overview_panel(&db, 1, 100, 200, None).await.expect("first save");
        save_overview_panel(&db, 1, 111, 222, None).await.expect("second save");

        let panels = all_overview_panels(&db).await.expect("list");
        assert_eq!(panels.len(), 1, "one overview panel per guild");
        assert_eq!((panels[0].channel_id, panels[0].message_id), (111, 222));
    }

    #[tokio::test]
    async fn remove_overview_panel_reports_whether_a_record_existed() {
        let db = fresh_db().await;
        save_overview_panel(&db, 1, 100, 200, None).await.expect("save");

        assert!(remove_overview_panel(&db, 1).await.expect("remove"));
        assert!(!remove_overview_panel(&db, 1).await.expect("remove again"));
        assert!(get_overview_panel(&db, 1).await.expect("get").is_none());
    }

    #[tokio::test]
    async fn remove_overview_panel_if_message_only_deletes_the_matching_record() {
        let db = fresh_db().await;
        save_overview_panel(&db, 1, 100, 200, None).await.expect("save");

        assert!(!remove_overview_panel_if_message(&db, 1, 999).await.expect("mismatch"));
        assert!(get_overview_panel(&db, 1).await.expect("get").is_some());
        assert!(remove_overview_panel_if_message(&db, 1, 200).await.expect("match"));
        assert!(get_overview_panel(&db, 1).await.expect("get").is_none());
    }

    #[tokio::test]
    async fn touch_overview_panel_bumps_updated_at_and_ignores_missing_rows() {
        let db = fresh_db().await;
        save_overview_panel(&db, 1, 100, 200, None).await.expect("save");
        let before = get_overview_panel(&db, 1).await.expect("get").expect("row").updated_at;

        touch_overview_panel(&db, 1).await.expect("touch");
        let after = get_overview_panel(&db, 1).await.expect("get").expect("row").updated_at;
        assert!(after >= before);

        touch_overview_panel(&db, 999).await.expect("touch missing row");
    }

    // --- overview_panels_for_effective_guild (cross-guild link fan-out) ---

    #[tokio::test]
    async fn overview_panels_for_effective_guild_includes_own_and_every_linked_one() {
        let db = fresh_db().await;
        save_overview_panel(&db, 1, 100, 200, None).await.expect("save A");
        save_overview_panel(&db, 2, 300, 400, Some(1)).await.expect("save B");
        save_overview_panel(&db, 3, 500, 600, Some(1)).await.expect("save C");
        save_overview_panel(&db, 4, 700, 800, Some(9)).await.expect("save D (different source)");

        let mut guild_ids: Vec<i64> = overview_panels_for_effective_guild(&db, 1)
            .await
            .unwrap()
            .iter()
            .map(|p| p.guild_id)
            .collect();
        guild_ids.sort();
        assert_eq!(guild_ids, vec![1, 2, 3]);
    }

    async fn insert_stale_panel(db: &DatabaseConnection, guild_id: i64) -> active_medals_panels::Model {
        insert_guild(db, guild_id).await;
        let past = now() - chrono::Duration::days(30);
        active_medals_panels::ActiveModel {
            id: Set(Uuid::now_v7()),
            guild_id: Set(guild_id),
            category: Set("Government".to_string()),
            channel_id: Set(100),
            message_id: Set(200),
            source_guild_id: Set(None),
            created_at: Set(past),
            updated_at: Set(past),
        }
        .insert(db)
        .await
        .expect("insert panel")
    }

    #[tokio::test]
    async fn successful_refresh_touches_the_record() {
        let db = fresh_db().await;
        let panel = insert_stale_panel(&db, 1).await;

        let fate = settle_refresh(&db, &panel, Ok(())).await.expect("settle");
        assert_eq!(fate, PanelFate::Updated);
        let after = get_panel(&db, 1, "Government").await.expect("get").expect("row").updated_at;
        assert!(after > panel.updated_at);
    }

    #[tokio::test]
    async fn dead_target_removes_the_record_f32() {
        let db = fresh_db().await;
        let panel = insert_stale_panel(&db, 1).await;

        let fate = settle_refresh(&db, &panel, Err(EditFailure::DeadTarget)).await.expect("settle");
        assert_eq!(fate, PanelFate::RemovedDead);
        assert!(get_panel(&db, 1, "Government").await.expect("get").is_none());
    }

    #[tokio::test]
    async fn dead_target_on_a_replaced_record_keeps_the_fresh_panel() {
        let db = fresh_db().await;
        let stale = insert_stale_panel(&db, 1).await;

        save_panel(&db, 1, "Government", 111, 222, None).await.expect("replace");

        let fate = settle_refresh(&db, &stale, Err(EditFailure::DeadTarget)).await.expect("settle");
        assert_eq!(fate, PanelFate::Superseded);
        let fresh = get_panel(&db, 1, "Government").await.expect("get").expect("row");
        assert_eq!((fresh.channel_id, fresh.message_id), (111, 222));
    }

    #[tokio::test]
    async fn transient_failure_keeps_the_record_untouched() {
        let db = fresh_db().await;
        let panel = insert_stale_panel(&db, 1).await;

        let fate = settle_refresh(&db, &panel, Err(EditFailure::Transient)).await.expect("settle");
        assert_eq!(fate, PanelFate::KeptAfterError);
        let row = get_panel(&db, 1, "Government").await.expect("get").expect("row");
        assert_eq!(row.updated_at, panel.updated_at);
    }

    // --- settle_refresh_overview (F31/F32 record lifecycle) ---

    async fn insert_stale_overview_panel(
        db: &DatabaseConnection,
        guild_id: i64,
    ) -> medals_overview_panels::Model {
        insert_guild(db, guild_id).await;
        let past = now() - chrono::Duration::days(30);
        medals_overview_panels::ActiveModel {
            guild_id: Set(guild_id),
            channel_id: Set(100),
            message_id: Set(200),
            source_guild_id: Set(None),
            created_at: Set(past),
            updated_at: Set(past),
        }
        .insert(db)
        .await
        .expect("insert panel")
    }

    #[tokio::test]
    async fn overview_successful_refresh_touches_the_record() {
        let db = fresh_db().await;
        let panel = insert_stale_overview_panel(&db, 1).await;

        let fate = settle_refresh_overview(&db, &panel, Ok(())).await.expect("settle");
        assert_eq!(fate, PanelFate::Updated);
        let after = get_overview_panel(&db, 1).await.expect("get").expect("row").updated_at;
        assert!(after > panel.updated_at);
    }

    #[tokio::test]
    async fn overview_dead_target_removes_the_record_f32() {
        let db = fresh_db().await;
        let panel = insert_stale_overview_panel(&db, 1).await;

        let fate =
            settle_refresh_overview(&db, &panel, Err(EditFailure::DeadTarget)).await.expect("settle");
        assert_eq!(fate, PanelFate::RemovedDead);
        assert!(get_overview_panel(&db, 1).await.expect("get").is_none());
    }

    #[tokio::test]
    async fn overview_dead_target_on_a_replaced_record_keeps_the_fresh_panel() {
        let db = fresh_db().await;
        let stale = insert_stale_overview_panel(&db, 1).await;

        save_overview_panel(&db, 1, 111, 222, None).await.expect("replace");

        let fate =
            settle_refresh_overview(&db, &stale, Err(EditFailure::DeadTarget)).await.expect("settle");
        assert_eq!(fate, PanelFate::Superseded);
        let fresh = get_overview_panel(&db, 1).await.expect("get").expect("row");
        assert_eq!((fresh.channel_id, fresh.message_id), (111, 222));
    }

    #[tokio::test]
    async fn overview_transient_failure_keeps_the_record_untouched() {
        let db = fresh_db().await;
        let panel = insert_stale_overview_panel(&db, 1).await;

        let fate =
            settle_refresh_overview(&db, &panel, Err(EditFailure::Transient)).await.expect("settle");
        assert_eq!(fate, PanelFate::KeptAfterError);
        let row = get_overview_panel(&db, 1).await.expect("get").expect("row");
        assert_eq!(row.updated_at, panel.updated_at);
    }

    // --- render_category_embed ---

    async fn insert_trophy(
        db: &DatabaseConnection,
        guild_id: i64,
        name: &str,
        category: Option<&str>,
        active: bool,
    ) {
        if guilds::Entity::find_by_id(guild_id).one(db).await.unwrap().is_none() {
            insert_guild(db, guild_id).await;
        }
        trophies::ActiveModel {
            id: Set(Uuid::now_v7()),
            guild_id: Set(guild_id),
            legacy_id: Set(None),
            creator_user_id: Set(None),
            name: Set(name.to_string()),
            normalized_name: Set(crate::domain::normalize::normalize_name(name)),
            description: Set(format!("{name} description")),
            emoji: Set("🏆".to_string()),
            value: Set(10),
            image: Set(None),
            dedication_user_id: Set(None),
            dedication_text: Set(None),
            details: Set("No details provided.".to_string()),
            signed: Set(false),
            category: Set(category.map(str::to_string)),
            active: Set(active),
            created_at: Set(now()),
            updated_at: Set(now()),
        }
        .insert(db)
        .await
        .expect("insert trophy");
    }

    #[tokio::test]
    async fn render_lists_only_active_medals_in_the_category_alphabetically() {
        let db = fresh_db().await;
        insert_trophy(&db, 1, "Zebra Medal", Some("Government"), true).await;
        insert_trophy(&db, 1, "Alpha Medal", Some("Government"), true).await;
        insert_trophy(&db, 1, "Inactive Medal", Some("Government"), false).await;
        insert_trophy(&db, 1, "Other Category", Some("Recurring"), true).await;
        insert_trophy(&db, 1, "Uncategorized", None, true).await;

        let locale = i18n::resolve(None);
        let embed = render_category_embed(&db, 1, "Government", &locale).await.expect("render");
        let json = serde_json::to_value(&embed).expect("serialize");
        let description = json["description"].as_str().unwrap();

        assert!(description.contains("Alpha Medal"));
        assert!(description.contains("Zebra Medal"));
        assert!(!description.contains("Inactive Medal"), "inactive medal must not appear");
        assert!(!description.contains("Other Category"));
        assert!(!description.contains("Uncategorized"));
        assert!(
            description.find("Alpha Medal").unwrap() < description.find("Zebra Medal").unwrap(),
            "alphabetical order: {description}"
        );
    }

    #[tokio::test]
    async fn render_shows_a_placeholder_when_nothing_is_active() {
        let db = fresh_db().await;
        insert_trophy(&db, 1, "Retired Medal", Some("Government"), false).await;

        let locale = i18n::resolve(None);
        let embed = render_category_embed(&db, 1, "Government", &locale).await.expect("render");
        let json = serde_json::to_value(&embed).expect("serialize");
        let description = json["description"].as_str().unwrap();
        assert_eq!(description, i18n::t(&locale, "medals-panel-empty"));
        assert_ne!(description, "medals-panel-empty", "catalog key must exist");
    }

    // --- render_overview_embed ---

    #[tokio::test]
    async fn overview_groups_active_categorized_medals_by_category_alphabetically() {
        let db = fresh_db().await;
        insert_trophy(&db, 1, "Zebra Medal", Some("Recurring"), true).await;
        insert_trophy(&db, 1, "Bronze Medal", Some("Government"), true).await;
        insert_trophy(&db, 1, "Gold Medal", Some("Government"), true).await;
        insert_trophy(&db, 1, "Retired Medal", Some("Government"), false).await;
        insert_trophy(&db, 1, "Loose Medal", None, true).await;

        let locale = i18n::resolve(None);
        let embed = render_overview_embed(&db, 1, &locale).await.expect("render");
        let json = serde_json::to_value(&embed).expect("serialize");
        let fields = json["fields"].as_array().expect("fields present");

        let names: Vec<&str> = fields.iter().map(|f| f["name"].as_str().unwrap()).collect();
        assert_eq!(names, vec!["Government", "Recurring"], "categories sorted alphabetically");

        let government = fields[0]["value"].as_str().unwrap();
        assert!(government.contains("Bronze Medal") && government.contains("Gold Medal"));
        assert!(!government.contains("Retired Medal"), "inactive medal excluded");
        assert!(!government.contains("Loose Medal"), "uncategorized medal excluded from overview");

        let recurring = fields[1]["value"].as_str().unwrap();
        assert!(recurring.contains("Zebra Medal"));
    }

    #[tokio::test]
    async fn overview_shows_a_placeholder_when_nothing_categorized_is_active() {
        let db = fresh_db().await;
        insert_trophy(&db, 1, "Uncategorized", None, true).await;
        insert_trophy(&db, 1, "Retired", Some("Government"), false).await;

        let locale = i18n::resolve(None);
        let embed = render_overview_embed(&db, 1, &locale).await.expect("render");
        let json = serde_json::to_value(&embed).expect("serialize");
        assert_eq!(json["description"].as_str().unwrap(), i18n::t(&locale, "medals-overview-empty"));
        assert!(json.get("fields").is_none() || json["fields"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn overview_caps_fields_at_the_discord_limit_and_notes_truncation() {
        let db = fresh_db().await;
        for i in 0..30 {
            insert_trophy(&db, 1, &format!("Medal {i:02}"), Some(&format!("Category {i:02}")), true)
                .await;
        }

        let locale = i18n::resolve(None);
        let embed = render_overview_embed(&db, 1, &locale).await.expect("render");
        let json = serde_json::to_value(&embed).expect("serialize");
        let fields = json["fields"].as_array().expect("fields present");
        assert_eq!(fields.len(), MAX_OVERVIEW_FIELDS);
        assert!(json["footer"]["text"].as_str().unwrap().len() > 0, "truncation footer present");
    }

    // --- distinct_categories ---

    #[tokio::test]
    async fn distinct_categories_are_alphabetical_and_guild_scoped() {
        let db = fresh_db().await;
        insert_trophy(&db, 1, "A", Some("Recurring"), true).await;
        insert_trophy(&db, 1, "B", Some("Government"), true).await;
        insert_trophy(&db, 1, "C", Some("Government"), true).await; // duplicate category
        insert_trophy(&db, 1, "D", None, true).await; // uncategorized, excluded
        insert_trophy(&db, 2, "E", Some("Other Guild"), true).await;

        let categories = distinct_categories(&db, 1).await.expect("query");
        assert_eq!(categories, vec!["Government".to_string(), "Recurring".to_string()]);
    }

    // --- i18n catalog ---

    #[test]
    fn catalog_keys_exist() {
        let locale = i18n::resolve(None);
        assert_ne!(
            i18n::t_args(&locale, "medals-panel-title", &[("category", "Government".into())]),
            "medals-panel-title"
        );
        assert_ne!(
            i18n::t_args(
                &locale,
                "medals-panel-row",
                &[
                    ("emoji", "🏆".into()),
                    ("name", "Gold".into()),
                    ("description", "desc".into())
                ]
            ),
            "medals-panel-row"
        );
        assert_ne!(i18n::t(&locale, "medals-panel-empty"), "medals-panel-empty");
        for key in [
            "medals-overview-title",
            "medals-overview-empty",
            "medals-overview-truncated",
            "medals-overview-more-categories",
        ] {
            assert_ne!(i18n::t(&locale, key), key, "missing catalog key {key}");
        }
    }
}
