//! `/rewards add|remove|clear|list` — role rewards management (batch C12).
//!
//! Spec: docs/specs/commands-admin.md §/rewards. Fixes applied:
//! - F21: the role-hierarchy check is real (the legacy one was dead code):
//!   non-owners cannot add/remove rewards for roles at or above their own
//!   highest role. Discord ordering: higher position wins; on a position
//!   tie the role with the LOWER id is higher (serenity `Role: Ord`).
//! - F22: duplicate reward roles are rejected with a clear error (legacy
//!   compared a stored id string against a Role object — always false);
//!   `UNIQUE(guild_id, role_id)` backs this at the DB level.
//! - F23: the limit is exactly 20 rewards (legacy off-by-one allowed 21).
//! - F24: `remove` operates on the STORED role id — the parameter is a
//!   string with autocomplete over the configured rewards (value = stored
//!   role id), also accepting a pasted mention or raw id, so rewards
//!   pointing at deleted roles can be removed; `list` marks deleted roles.
//!   The parameter-type change vs the legacy Role option is a documented
//!   intentional delta (rust-parity-plan.md §4).
//! - F25: correct command/subcommand descriptions (legacy source shipped
//!   copy-paste strings like "Add permissions to a role.").
//!
//! Parity kept: requirement minimum 1; duplicate requirement values
//! rejected; rewards listed 5 per page sorted by requirement descending
//! with the caller's score in the description; removing/clearing rewards
//! never retro-updates members' roles (stated in the remove footer) — role
//! reconciliation happens on score changes (award/revoke/clear batch).
//!
//! DELIBERATE SPEC-CONFLICT RESOLUTION (reviewed): commands-admin.md
//! §/rewards "Rust target" asks to "apply/reconcile roles ... whenever
//! rewards change", but rust-parity-plan.md wins here:
//! - §2 scopes the reward engine to award/revoke/clear;
//! - §3 F21–F25 is the complete /rewards fix list and excludes it;
//! - §4 delta 4 states roles appear "on their first score change";
//! - Principle 3: absent an ADR-backed delta, legacy behavior wins — and
//!   legacy QUIRK (commands-admin.md, rewards.js:176-182) is exactly this,
//!   surfaced to users in the remove footer.
//!
//! A guild-wide reconcile inside a slash-command handler would also be an
//! unbounded fan-out of role API calls; if wanted, it belongs to the
//! background-work batch (§3.4) next to the panel sweep, not here.
//!
//! New: explicit empty-state message on `list` (the legacy zero-width-space
//! trick would be a 400 on Discord's current API).

use std::collections::HashSet;

use poise::serenity_prelude as serenity;
use sea_orm::sea_query::OnConflict;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder, Set,
    TransactionTrait,
};
use uuid::Uuid;

use crate::bot::util::{self, paginate};
use crate::bot::{Context, Error};
use crate::domain::queries;
use crate::entities::{guilds, role_rewards};
use crate::i18n::{self, LanguageIdentifier};

/// Exactly 20 rewards per guild (F23).
pub(crate) const MAX_REWARDS: usize = 20;
/// Rows per page on `list` (legacy `getPage(list, 5, page)`).
pub(crate) const PER_PAGE: usize = 5;
/// Requirements are stored as `i32` (`role_rewards.requirement`).
pub(crate) const MAX_REQUIREMENT: i64 = i32::MAX as i64;

/// Manage the role rewards of your server.
#[poise::command(
    slash_command,
    guild_only,
    default_member_permissions = "MANAGE_GUILD",
    required_permissions = "MANAGE_GUILD",
    subcommands("add", "remove", "clear", "list"),
    subcommand_required
)]
pub async fn rewards(_ctx: Context<'_>) -> Result<(), Error> {
    // Never reached: subcommand_required.
    Ok(())
}

/// Add a role reward to your server.
#[poise::command(slash_command, guild_only)]
async fn add(
    ctx: Context<'_>,
    #[description = "Which role you want to add as reward."] role: serenity::Role,
    #[description = "How much score the user will require to get this role."]
    #[min = 1]
    requirement: i64,
) -> Result<(), Error> {
    let guild_id = require_guild_id(&ctx)?;
    let locale = util::locale(&ctx);
    let mention = format!("<@&{}>", role.id.get());

    let requirement = match validate_requirement(requirement) {
        Ok(value) => value,
        Err(RequirementError::TooLow) => {
            return util::reply_error(ctx, i18n::t(&locale, "rewards-add-error-requirement"), true)
                .await;
        }
        Err(RequirementError::TooLarge) => {
            return util::reply_error(
                ctx,
                i18n::t_args(
                    &locale,
                    "rewards-add-error-requirement-too-large",
                    &[("max", MAX_REQUIREMENT.into())],
                ),
                true,
            )
            .await;
        }
    };

    // F21: real hierarchy check.
    let guild = require_partial_guild(&ctx).await?;
    let hierarchy = hierarchy_context(&ctx, &guild).await?;
    let target = (role.position, role.id.get());
    if !hierarchy_allows(hierarchy.is_owner, hierarchy.caller_top, target) {
        return util::reply_error(
            ctx,
            i18n::t_args(&locale, "rewards-error-hierarchy", &[("role", mention.into())]),
            true,
        )
        .await;
    }

    let role_id = i64::try_from(role.id.get())?;
    match add_reward(&ctx.data().db, guild_id, role_id, requirement).await? {
        AddOutcome::Added => {
            log::info!(
                "rewards: guild {guild_id} added reward role {role_id} at requirement \
                 {requirement} (by user {})",
                ctx.author().id
            );
            let embed = serenity::CreateEmbed::new()
                .description(i18n::t_args(
                    &locale,
                    "rewards-add-success",
                    &[("requirement", requirement.into()), ("role", mention.into())],
                ))
                .colour(util::COLOR_SUCCESS);
            util::reply_embed(ctx, embed, false).await
        }
        AddOutcome::LimitReached => {
            util::reply_error(
                ctx,
                i18n::t_args(
                    &locale,
                    "rewards-add-error-limit",
                    &[("max", (MAX_REWARDS as i64).into())],
                ),
                true,
            )
            .await
        }
        AddOutcome::DuplicateRole => {
            util::reply_error(
                ctx,
                i18n::t_args(
                    &locale,
                    "rewards-add-error-duplicate-role",
                    &[("role", mention.into())],
                ),
                true,
            )
            .await
        }
        AddOutcome::DuplicateRequirement => {
            util::reply_error(
                ctx,
                i18n::t_args(
                    &locale,
                    "rewards-add-error-duplicate-requirement",
                    &[("requirement", requirement.into())],
                ),
                true,
            )
            .await
        }
    }
}

/// Remove a role reward from your server.
#[poise::command(slash_command, guild_only)]
async fn remove(
    ctx: Context<'_>,
    #[description = "The reward role to remove. Pick a suggestion, or paste a mention/ID (works for deleted roles)."]
    #[autocomplete = "autocomplete_reward_role"]
    role: String,
) -> Result<(), Error> {
    let guild_id = require_guild_id(&ctx)?;
    let locale = util::locale(&ctx);

    let Some(role_id) = parse_role_ref(&role) else {
        return util::reply_error(
            ctx,
            i18n::t(&locale, "rewards-remove-error-invalid-role"),
            true,
        )
        .await;
    };
    let mention = format!("<@&{role_id}>");

    // F21 applies to remove too — but only when the role still exists
    // (F24: rewards for deleted roles must always be removable).
    let guild = require_partial_guild(&ctx).await?;
    if let Some(existing) = guild.roles.get(&serenity::RoleId::new(role_id as u64)) {
        let hierarchy = hierarchy_context(&ctx, &guild).await?;
        let target = (existing.position, existing.id.get());
        if !hierarchy_allows(hierarchy.is_owner, hierarchy.caller_top, target) {
            return util::reply_error(
                ctx,
                i18n::t_args(
                    &locale,
                    "rewards-error-hierarchy",
                    &[("role", mention.into())],
                ),
                true,
            )
            .await;
        }
    }

    match remove_reward(&ctx.data().db, guild_id, role_id).await? {
        RemoveOutcome::Removed => {
            log::info!(
                "rewards: guild {guild_id} removed reward role {role_id} (by user {})",
                ctx.author().id
            );
            let embed = serenity::CreateEmbed::new()
                .description(i18n::t_args(
                    &locale,
                    "rewards-remove-success",
                    &[("role", mention.into())],
                ))
                .footer(serenity::CreateEmbedFooter::new(i18n::t(
                    &locale,
                    "rewards-remove-footer",
                )))
                .colour(util::COLOR_SUCCESS);
            util::reply_embed(ctx, embed, false).await
        }
        RemoveOutcome::NoRewards => {
            util::reply_error(ctx, i18n::t(&locale, "rewards-error-no-rewards"), true).await
        }
        RemoveOutcome::NotAReward => {
            util::reply_error(
                ctx,
                i18n::t_args(
                    &locale,
                    "rewards-remove-error-not-a-reward",
                    &[("role", mention.into())],
                ),
                true,
            )
            .await
        }
    }
}

/// Clears all rewards in this server.
#[poise::command(slash_command, guild_only)]
async fn clear(ctx: Context<'_>) -> Result<(), Error> {
    let guild_id = require_guild_id(&ctx)?;
    let locale = util::locale(&ctx);

    match clear_rewards(&ctx.data().db, guild_id).await? {
        ClearOutcome::NoRewards => {
            util::reply_error(ctx, i18n::t(&locale, "rewards-error-no-rewards"), true).await
        }
        ClearOutcome::Cleared(count) => {
            log::info!(
                "rewards: guild {guild_id} cleared {count} reward roles (by user {})",
                ctx.author().id
            );
            let embed = serenity::CreateEmbed::new()
                .description(i18n::t_args(
                    &locale,
                    "rewards-clear-success",
                    &[("count", (count as i64).into())],
                ))
                .colour(util::COLOR_SUCCESS);
            util::reply_embed(ctx, embed, false).await
        }
    }
}

/// List of reward roles.
#[poise::command(slash_command, guild_only)]
async fn list(
    ctx: Context<'_>,
    #[description = "Page to look at"] page: Option<i64>,
) -> Result<(), Error> {
    let guild_id = require_guild_id(&ctx)?;
    let locale = util::locale(&ctx);
    let db = &ctx.data().db;
    let author_id = i64::try_from(ctx.author().id.get())?;

    // F24: mark deleted roles. `None` (guild unavailable) marks nothing —
    // better to skip the marker than to flag every reward as deleted.
    let guild = ctx.partial_guild().await;
    let guild_name = guild
        .as_ref()
        .map(|guild| guild.name.clone())
        .unwrap_or_else(|| i18n::t(&locale, "rewards-list-fallback-guild-name"));
    let existing_roles: Option<HashSet<i64>> = guild
        .as_ref()
        .map(|guild| guild.roles.keys().map(|id| id.get() as i64).collect());

    let score = queries::user_score(db, guild_id, author_id).await?;
    let rewards = list_rewards(db, guild_id).await?;
    let entries: Vec<RewardEntry> = rewards
        .iter()
        .map(|reward| RewardEntry {
            role_id: reward.role_id,
            requirement: reward.requirement,
            exists: existing_roles
                .as_ref()
                .is_none_or(|roles| roles.contains(&reward.role_id)),
        })
        .collect();

    let (slice, current, last) = paginate(&entries, PER_PAGE, page.unwrap_or(1));
    let body = if slice.is_empty() {
        i18n::t(&locale, "rewards-list-empty")
    } else {
        render_entries(&locale, slice)
    };

    let embed = serenity::CreateEmbed::new()
        .title(i18n::t_args(
            &locale,
            "rewards-list-title",
            &[("guild", guild_name.into())],
        ))
        .description(format!(
            "{}\n\n{body}",
            i18n::t_args(&locale, "rewards-list-description", &[("score", score.into())])
        ))
        .footer(serenity::CreateEmbedFooter::new(i18n::t_args(
            &locale,
            "rewards-list-footer-page",
            &[("page", current.into()), ("last", last.into())],
        )))
        .colour(util::COLOR_MAIN);

    util::reply_embed(ctx, embed, false).await
}

// ---------------------------------------------------------------------------
// Pure business logic (testable, no Discord types)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RequirementError {
    TooLow,
    TooLarge,
}

/// Legacy rule: requirement must be at least 1. New: cap at `i32::MAX`
/// (the storage type) instead of silently overflowing.
pub(crate) fn validate_requirement(requirement: i64) -> Result<i32, RequirementError> {
    if requirement < 1 {
        Err(RequirementError::TooLow)
    } else if requirement > MAX_REQUIREMENT {
        Err(RequirementError::TooLarge)
    } else {
        Ok(requirement as i32)
    }
}

/// Whether role `a` is STRICTLY below role `b` in Discord's hierarchy:
/// lower position, or — on a position tie — the higher snowflake id
/// (serenity `Role: Ord`: "Discord does position DESC, id ASC").
pub(crate) fn is_role_below(a: (u16, u64), b: (u16, u64)) -> bool {
    a.0 < b.0 || (a.0 == b.0 && a.1 > b.1)
}

/// F21: owners manage any role; everyone else only roles strictly below
/// their own highest role.
pub(crate) fn hierarchy_allows(is_owner: bool, caller_top: (u16, u64), target: (u16, u64)) -> bool {
    is_owner || is_role_below(target, caller_top)
}

/// Pure choice builder behind [`autocomplete_reward_role`]: filters the
/// guild's configured rewards (`(role_id, requirement, live role name)` —
/// `None` name = the role no longer exists) by the partial input and returns
/// `(label, value)` pairs. The VALUE is always the raw role id string, which
/// [`parse_role_ref`] accepts, so picking a suggestion works for deleted
/// roles too (F24). Matching is case-insensitive on the role name and
/// prefix-based on the id (pasted `<@&…>` mentions are unwrapped first).
pub(crate) fn reward_remove_choices(
    locale: &LanguageIdentifier,
    rewards: &[(i64, i32, Option<String>)],
    partial: &str,
) -> Vec<(String, String)> {
    let needle = partial.trim();
    let needle = needle
        .strip_prefix("<@&")
        .and_then(|rest| rest.strip_suffix('>'))
        .unwrap_or(needle)
        .to_lowercase();

    rewards
        .iter()
        .filter(|(role_id, _, name)| {
            needle.is_empty()
                || name.as_ref().is_some_and(|n| n.to_lowercase().contains(&needle))
                || role_id.to_string().starts_with(&needle)
        })
        .map(|(role_id, requirement, name)| {
            let label = match name {
                Some(name) => i18n::t_args(
                    locale,
                    "rewards-remove-choice",
                    &[("name", name.clone().into()), ("requirement", (*requirement).into())],
                ),
                None => i18n::t_args(
                    locale,
                    "rewards-remove-choice-deleted",
                    &[("id", role_id.to_string().into()), ("requirement", (*requirement).into())],
                ),
            };
            // Discord caps choice labels at 100 characters.
            (label.chars().take(100).collect(), role_id.to_string())
        })
        .take(crate::bot::resolver::MAX_CHOICES)
        .collect()
}

/// Poise autocomplete callback for the `/rewards remove` role option: offers
/// the guild's CONFIGURED rewards (max 20, so never truncated by Discord's
/// 25-choice cap), labeled with the live role name — or marked deleted when
/// the role is gone — while the sent value stays the stored role id (F24).
/// Autocomplete cannot report errors, so failures log and yield no choices;
/// the user can still paste a mention or raw id.
async fn autocomplete_reward_role(
    ctx: Context<'_>,
    partial: &str,
) -> Vec<serenity::AutocompleteChoice> {
    let Some(guild_id) = ctx.guild_id() else {
        return Vec::new();
    };
    let locale = util::locale(&ctx);
    let rewards = match list_rewards(&ctx.data().db, guild_id.get() as i64).await {
        Ok(rewards) => rewards,
        Err(err) => {
            log::warn!(
                "reward-role autocomplete query failed (guild={}): {err:#}",
                guild_id.get()
            );
            return Vec::new();
        }
    };

    // Cache access after every await: the guard must not live across one.
    // With the guild uncached the deleted-marker cannot be decided, so the
    // raw id doubles as the display name instead of flagging everything.
    let live_names: Option<std::collections::HashMap<i64, String>> = ctx.guild().map(|guild| {
        guild
            .roles
            .iter()
            .map(|(id, role)| (id.get() as i64, role.name.clone()))
            .collect()
    });
    let candidates: Vec<(i64, i32, Option<String>)> = rewards
        .iter()
        .map(|reward| {
            let name = match &live_names {
                Some(names) => names.get(&reward.role_id).cloned(),
                None => Some(reward.role_id.to_string()),
            };
            (reward.role_id, reward.requirement, name)
        })
        .collect();

    reward_remove_choices(&locale, &candidates, partial)
        .into_iter()
        .map(|(label, value)| serenity::AutocompleteChoice::new(label, value))
        .collect()
}

/// Parses a role reference: a mention (`<@&123>`) or a raw id (`123`).
pub(crate) fn parse_role_ref(input: &str) -> Option<i64> {
    let trimmed = input.trim();
    let digits = trimmed
        .strip_prefix("<@&")
        .and_then(|rest| rest.strip_suffix('>'))
        .unwrap_or(trimmed);
    // Discord snowflakes are positive and fit in i64.
    digits.parse::<i64>().ok().filter(|id| *id > 0)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AddOutcome {
    Added,
    LimitReached,
    DuplicateRole,
    DuplicateRequirement,
}

/// Validation against the existing `(role_id, requirement)` rewards, in the
/// legacy check order: limit first (F23: exactly 20), then duplicates
/// (F22: duplicate ROLE is now actually caught).
pub(crate) fn check_add(existing: &[(i64, i32)], role_id: i64, requirement: i32) -> AddOutcome {
    if existing.len() >= MAX_REWARDS {
        AddOutcome::LimitReached
    } else if existing.iter().any(|&(existing_role, _)| existing_role == role_id) {
        AddOutcome::DuplicateRole
    } else if existing.iter().any(|&(_, existing_req)| existing_req == requirement) {
        AddOutcome::DuplicateRequirement
    } else {
        AddOutcome::Added
    }
}

/// One row of the `list` output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RewardEntry {
    pub role_id: i64,
    pub requirement: i32,
    /// `false` marks a deleted role (F24).
    pub exists: bool,
}

/// Legacy layout: `**🏅 {requirement}**` over the role mention, entries
/// double-spaced; deleted roles get a marker (F24).
pub(crate) fn render_entries(locale: &LanguageIdentifier, entries: &[RewardEntry]) -> String {
    entries
        .iter()
        .map(|entry| {
            let mention = format!("<@&{}>", entry.role_id);
            let role = if entry.exists {
                mention
            } else {
                format!("{mention} {}", i18n::t(locale, "rewards-list-deleted-marker"))
            };
            i18n::t_args(
                locale,
                "rewards-list-entry",
                &[("requirement", entry.requirement.into()), ("role", role.into())],
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

// ---------------------------------------------------------------------------
// Database operations (testable against sqlite::memory:)
// ---------------------------------------------------------------------------

/// Validates and inserts a reward atomically. Auto-registers the guild row
/// (FK) like the other write paths do.
pub(crate) async fn add_reward(
    db: &DatabaseConnection,
    guild_id: i64,
    role_id: i64,
    requirement: i32,
) -> anyhow::Result<AddOutcome> {
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

    let existing: Vec<(i64, i32)> = role_rewards::Entity::find()
        .filter(role_rewards::Column::GuildId.eq(guild_id))
        .all(&txn)
        .await?
        .into_iter()
        .map(|reward| (reward.role_id, reward.requirement))
        .collect();

    let outcome = check_add(&existing, role_id, requirement);
    if outcome == AddOutcome::Added {
        role_rewards::ActiveModel {
            id: Set(Uuid::now_v7()),
            guild_id: Set(guild_id),
            role_id: Set(role_id),
            requirement: Set(requirement),
            created_at: Set(now),
            updated_at: Set(now),
        }
        .insert(&txn)
        .await?;
        txn.commit().await?;
    }
    Ok(outcome)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RemoveOutcome {
    Removed,
    /// The guild has no rewards at all (legacy distinct error).
    NoRewards,
    /// The guild has rewards, but not for this role.
    NotAReward,
}

/// F24: removal keyed on the STORED role id — no role cache involved.
pub(crate) async fn remove_reward(
    db: &DatabaseConnection,
    guild_id: i64,
    role_id: i64,
) -> anyhow::Result<RemoveOutcome> {
    let existing = role_rewards::Entity::find()
        .filter(role_rewards::Column::GuildId.eq(guild_id))
        .all(db)
        .await?;
    if existing.is_empty() {
        return Ok(RemoveOutcome::NoRewards);
    }
    if !existing.iter().any(|reward| reward.role_id == role_id) {
        return Ok(RemoveOutcome::NotAReward);
    }

    role_rewards::Entity::delete_many()
        .filter(role_rewards::Column::GuildId.eq(guild_id))
        .filter(role_rewards::Column::RoleId.eq(role_id))
        .exec(db)
        .await?;
    Ok(RemoveOutcome::Removed)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ClearOutcome {
    Cleared(u64),
    NoRewards,
}

/// Deletes every reward of the guild; legacy errors when there is nothing
/// to clear.
pub(crate) async fn clear_rewards(
    db: &DatabaseConnection,
    guild_id: i64,
) -> anyhow::Result<ClearOutcome> {
    let result = role_rewards::Entity::delete_many()
        .filter(role_rewards::Column::GuildId.eq(guild_id))
        .exec(db)
        .await?;
    if result.rows_affected == 0 {
        Ok(ClearOutcome::NoRewards)
    } else {
        Ok(ClearOutcome::Cleared(result.rows_affected))
    }
}

/// All rewards of a guild, requirement descending (legacy sort), role id
/// ascending on ties for determinism.
pub(crate) async fn list_rewards(
    db: &DatabaseConnection,
    guild_id: i64,
) -> anyhow::Result<Vec<role_rewards::Model>> {
    Ok(role_rewards::Entity::find()
        .filter(role_rewards::Column::GuildId.eq(guild_id))
        .order_by_desc(role_rewards::Column::Requirement)
        .order_by_asc(role_rewards::Column::RoleId)
        .all(db)
        .await?)
}

// ---------------------------------------------------------------------------
// Discord-context helpers (thin, not unit-tested)
// ---------------------------------------------------------------------------

struct HierarchyContext {
    is_owner: bool,
    /// `(position, id)` of the caller's highest role; `(0, guild_id)`
    /// (@everyone) when they have none.
    caller_top: (u16, u64),
}

async fn require_partial_guild(ctx: &Context<'_>) -> Result<serenity::PartialGuild, Error> {
    ctx.partial_guild()
        .await
        .ok_or_else(|| anyhow::anyhow!("could not resolve the guild for a /rewards command"))
}

async fn hierarchy_context(
    ctx: &Context<'_>,
    guild: &serenity::PartialGuild,
) -> Result<HierarchyContext, Error> {
    let member = ctx
        .author_member()
        .await
        .ok_or_else(|| anyhow::anyhow!("could not resolve the invoking member"))?;
    let everyone = (0u16, guild.id.get());
    let caller_top = member
        .roles
        .iter()
        .filter_map(|role_id| guild.roles.get(role_id))
        .map(|role| (role.position, role.id.get()))
        .fold(everyone, |best, candidate| {
            if is_role_below(best, candidate) { candidate } else { best }
        });
    Ok(HierarchyContext { is_owner: guild.owner_id == ctx.author().id, caller_top })
}

fn require_guild_id(ctx: &Context<'_>) -> Result<i64, Error> {
    Ok(util::require_guild_id(ctx)?.get() as i64)
}

#[cfg(test)]
mod tests {
    use sea_orm::EntityTrait;

    use super::*;
    use crate::domain::test_support::{fresh_db, insert_guild};

    fn en() -> LanguageIdentifier {
        "en-US".parse().unwrap()
    }

    // ---- pure logic ------------------------------------------------------

    #[test]
    fn requirement_must_be_at_least_one() {
        assert_eq!(validate_requirement(0), Err(RequirementError::TooLow));
        assert_eq!(validate_requirement(-5), Err(RequirementError::TooLow));
        assert_eq!(validate_requirement(1), Ok(1));
        assert_eq!(validate_requirement(999_999), Ok(999_999));
    }

    #[test]
    fn requirement_above_i32_max_is_rejected_not_truncated() {
        assert_eq!(validate_requirement(MAX_REQUIREMENT), Ok(i32::MAX));
        assert_eq!(
            validate_requirement(MAX_REQUIREMENT + 1),
            Err(RequirementError::TooLarge)
        );
    }

    #[test]
    fn f21_owner_bypasses_hierarchy() {
        assert!(hierarchy_allows(true, (0, 1), (100, 2)));
    }

    #[test]
    fn f21_non_owner_needs_target_strictly_below_highest_role() {
        let caller_top = (5, 10);
        assert!(hierarchy_allows(false, caller_top, (3, 11)), "below → allowed");
        assert!(!hierarchy_allows(false, caller_top, (5, 10)), "same role → blocked");
        assert!(!hierarchy_allows(false, caller_top, (7, 12)), "above → blocked");
    }

    #[test]
    fn f21_position_tie_breaks_on_snowflake_lower_id_is_higher() {
        let caller_top = (5, 10);
        // Same position, higher id → the target is BELOW the caller's role.
        assert!(hierarchy_allows(false, caller_top, (5, 20)));
        // Same position, lower id → the target is ABOVE the caller's role.
        assert!(!hierarchy_allows(false, caller_top, (5, 5)));
    }

    // ---- /rewards remove autocomplete (F24 + §4 delta) --------------------

    fn choice_fixtures() -> Vec<(i64, i32, Option<String>)> {
        vec![
            (100, 10, Some("Bronze Tier".to_string())),
            (200, 50, Some("Silver Tier".to_string())),
            (300, 90, None), // deleted role
        ]
    }

    #[test]
    fn remove_choices_value_is_always_the_stored_role_id() {
        let choices = reward_remove_choices(&en(), &choice_fixtures(), "");
        let values: Vec<&str> = choices.iter().map(|(_, value)| value.as_str()).collect();
        assert_eq!(values, vec!["100", "200", "300"]);
        for (label, value) in &choices {
            assert!(parse_role_ref(value).is_some(), "value {value} must parse");
            assert!(!label.is_empty() && label.chars().count() <= 100);
        }
    }

    #[test]
    fn remove_choices_filter_by_name_case_insensitively() {
        let choices = reward_remove_choices(&en(), &choice_fixtures(), "silver");
        assert_eq!(choices.len(), 1);
        assert!(choices[0].0.contains("Silver Tier"), "label: {}", choices[0].0);
        assert_eq!(choices[0].1, "200");
        // Substring, not only prefix.
        assert_eq!(reward_remove_choices(&en(), &choice_fixtures(), "TIER").len(), 2);
    }

    #[test]
    fn remove_choices_filter_by_id_prefix_and_unwrap_mentions() {
        // A deleted role has no name; it is findable by id (or listed blank).
        let by_id = reward_remove_choices(&en(), &choice_fixtures(), "30");
        assert_eq!(by_id.len(), 1);
        assert_eq!(by_id[0].1, "300");
        let by_mention = reward_remove_choices(&en(), &choice_fixtures(), "<@&300>");
        assert_eq!(by_mention.len(), 1);
        assert_eq!(by_mention[0].1, "300");
    }

    #[test]
    fn remove_choices_mark_deleted_roles_and_show_requirements() {
        let choices = reward_remove_choices(&en(), &choice_fixtures(), "");
        assert!(choices[0].0.contains("10"), "requirement shown: {}", choices[0].0);
        let deleted_marker = choices[2].0.to_lowercase();
        assert!(deleted_marker.contains("deleted"), "deleted marked: {}", choices[2].0);
        assert!(choices[2].0.contains("300"), "deleted labeled by id: {}", choices[2].0);
        assert!(
            !choices[0].0.to_lowercase().contains("deleted"),
            "live roles are not flagged: {}",
            choices[0].0
        );
    }

    #[test]
    fn remove_choices_no_match_yields_no_choices() {
        assert!(reward_remove_choices(&en(), &choice_fixtures(), "nope").is_empty());
        assert!(reward_remove_choices(&en(), &[], "").is_empty());
    }

    /// The `/rewards remove` parameter delta (String + autocomplete instead
    /// of the legacy Role option, required by F24) is user-visible and MUST
    /// stay listed in rust-parity-plan.md §4. If this fails you changed the
    /// parameter or removed the docs entry.
    #[test]
    fn remove_parameter_delta_is_documented_in_the_parity_plan() {
        let plan = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/docs/specs/rust-parity-plan.md"
        ))
        .expect("read rust-parity-plan.md");
        let deltas = plan
            .split("## 4.")
            .nth(1)
            .expect("parity plan has a §4 intentional-deltas section");
        assert!(
            deltas.contains("`/rewards remove`"),
            "§4 must list the /rewards remove parameter delta"
        );
        assert!(
            deltas.contains("autocomplete over the configured rewards"),
            "§4 must describe the autocomplete replacement"
        );
    }

    #[test]
    fn parse_role_ref_accepts_mentions_and_raw_ids() {
        assert_eq!(parse_role_ref("<@&123456789>"), Some(123_456_789));
        assert_eq!(parse_role_ref("  123456789 "), Some(123_456_789));
        assert_eq!(parse_role_ref("not a role"), None);
        assert_eq!(parse_role_ref("<@&nope>"), None);
        assert_eq!(parse_role_ref("<@123>"), None, "user mentions are not roles");
        assert_eq!(parse_role_ref("-5"), None);
        assert_eq!(parse_role_ref(""), None);
    }

    #[test]
    fn check_add_orders_limit_then_duplicate_role_then_duplicate_requirement() {
        let existing = vec![(1i64, 10i32), (2, 20)];
        assert_eq!(check_add(&existing, 3, 30), AddOutcome::Added);
        assert_eq!(check_add(&existing, 1, 30), AddOutcome::DuplicateRole);
        assert_eq!(check_add(&existing, 3, 20), AddOutcome::DuplicateRequirement);
        // Same role AND same requirement → the role duplicate wins (clearer).
        assert_eq!(check_add(&existing, 2, 20), AddOutcome::DuplicateRole);
    }

    #[test]
    fn f23_check_add_blocks_the_21st_reward() {
        let existing: Vec<(i64, i32)> = (0..MAX_REWARDS as i64).map(|i| (i, i as i32 + 1)).collect();
        assert_eq!(existing.len(), 20);
        assert_eq!(check_add(&existing, 999, 999), AddOutcome::LimitReached);
        let nineteen = &existing[..19];
        assert_eq!(check_add(nineteen, 999, 999), AddOutcome::Added, "the 20th is allowed");
    }

    #[test]
    fn render_entries_shows_requirement_mention_and_deleted_marker() {
        let locale = en();
        let entries = [
            RewardEntry { role_id: 42, requirement: 100, exists: true },
            RewardEntry { role_id: 7, requirement: 10, exists: false },
        ];
        let body = render_entries(&locale, &entries);
        assert!(body.contains("100"), "requirement rendered: {body}");
        assert!(body.contains("<@&42>"), "role mention rendered: {body}");
        assert!(body.contains("🏅"), "medal rendered: {body}");
        assert!(body.contains("<@&7>"), "deleted role still mentioned: {body}");
        let marker = i18n::t(&en(), "rewards-list-deleted-marker");
        assert_eq!(body.matches(&marker).count(), 1, "only the deleted role is marked: {body}");
        assert!(body.contains("\n\n"), "entries are double-spaced: {body}");
    }

    #[test]
    fn catalog_has_all_rewards_keys() {
        let locale = en();
        // Argless messages: `t` must resolve them.
        for key in [
            "rewards-add-error-requirement",
            "rewards-error-no-rewards",
            "rewards-remove-footer",
            "rewards-remove-error-invalid-role",
            "rewards-list-fallback-guild-name",
            "rewards-list-deleted-marker",
            "rewards-list-empty",
        ] {
            assert_ne!(i18n::t(&locale, key), key, "missing catalog key {key}");
        }
        // Messages with placeables: resolve with their arguments provided
        // (fluent-templates returns None when required args are missing).
        let role = || ("role", "<@&1>".into());
        let requirement = || ("requirement", 100.into());
        let with_args: &[(&str, Vec<(&'static str, i18n::FluentValue<'static>)>)] = &[
            ("rewards-remove-choice", vec![("name", "Silver".into()), requirement()]),
            ("rewards-remove-choice-deleted", vec![("id", "300".into()), requirement()]),
            ("rewards-add-success", vec![requirement(), role()]),
            ("rewards-add-error-requirement-too-large", vec![("max", MAX_REQUIREMENT.into())]),
            ("rewards-add-error-limit", vec![("max", 20.into())]),
            ("rewards-add-error-duplicate-role", vec![role()]),
            ("rewards-add-error-duplicate-requirement", vec![requirement()]),
            ("rewards-error-hierarchy", vec![role()]),
            ("rewards-remove-success", vec![role()]),
            ("rewards-remove-error-not-a-reward", vec![role()]),
            ("rewards-clear-success", vec![("count", 2.into())]),
            ("rewards-list-title", vec![("guild", "Guild".into())]),
            ("rewards-list-description", vec![("score", 5.into())]),
            ("rewards-list-entry", vec![requirement(), role()]),
            ("rewards-list-footer-page", vec![("page", 1.into()), ("last", 2.into())]),
        ];
        for (key, args) in with_args {
            assert_ne!(i18n::t_args(&locale, key, args), *key, "missing catalog key {key}");
        }
    }

    #[test]
    fn clear_success_pluralizes() {
        let locale = en();
        let one = i18n::t_args(&locale, "rewards-clear-success", &[("count", 1.into())]);
        let many = i18n::t_args(&locale, "rewards-clear-success", &[("count", 3.into())]);
        assert!(one.contains("only role reward"), "{one}");
        assert!(many.contains('3'), "{many}");
    }

    #[test]
    fn f25_descriptions_are_corrected() {
        let command = rewards();
        assert_eq!(
            command.description.as_deref(),
            Some("Manage the role rewards of your server."),
            "top-level description must not be the legacy copy-paste"
        );
        let add = command.subcommands.iter().find(|c| c.name == "add").expect("add subcommand");
        let description = add.description.as_deref().expect("add description");
        assert!(
            description.contains("reward") && !description.contains("permissions"),
            "add description must not be 'Add permissions to a role.': {description}"
        );
        for name in ["remove", "clear", "list"] {
            assert!(
                command.subcommands.iter().any(|c| c.name == name),
                "missing subcommand {name}"
            );
        }
    }

    // ---- database operations ---------------------------------------------

    #[tokio::test]
    async fn add_reward_inserts_and_auto_registers_the_guild() {
        let db = fresh_db().await;
        // No insert_guild on purpose: add must create the FK row itself.
        let outcome = add_reward(&db, 1, 42, 100).await.expect("add");
        assert_eq!(outcome, AddOutcome::Added);

        let guild = guilds::Entity::find_by_id(1).one(&db).await.expect("query");
        assert!(guild.is_some(), "guild row auto-registered");

        let rewards = list_rewards(&db, 1).await.expect("list");
        assert_eq!(rewards.len(), 1);
        assert_eq!((rewards[0].role_id, rewards[0].requirement), (42, 100));
    }

    #[tokio::test]
    async fn add_reward_does_not_clobber_an_existing_guild_row() {
        let db = fresh_db().await;
        insert_guild(&db, 1).await; // is_safe = true
        add_reward(&db, 1, 42, 100).await.expect("add");
        let guild = guilds::Entity::find_by_id(1).one(&db).await.expect("query").expect("row");
        assert!(guild.is_safe, "upsert must not overwrite the existing guild row");
    }

    #[tokio::test]
    async fn f22_add_reward_rejects_duplicate_role_with_clear_outcome() {
        let db = fresh_db().await;
        add_reward(&db, 1, 42, 100).await.expect("add");
        let outcome = add_reward(&db, 1, 42, 200).await.expect("add duplicate role");
        assert_eq!(outcome, AddOutcome::DuplicateRole);
        assert_eq!(list_rewards(&db, 1).await.expect("list").len(), 1, "nothing inserted");
    }

    #[tokio::test]
    async fn add_reward_rejects_duplicate_requirement() {
        let db = fresh_db().await;
        add_reward(&db, 1, 42, 100).await.expect("add");
        let outcome = add_reward(&db, 1, 43, 100).await.expect("add duplicate requirement");
        assert_eq!(outcome, AddOutcome::DuplicateRequirement);
        assert_eq!(list_rewards(&db, 1).await.expect("list").len(), 1);
    }

    #[tokio::test]
    async fn f23_add_reward_enforces_exactly_twenty() {
        let db = fresh_db().await;
        for i in 0..MAX_REWARDS as i64 {
            let outcome = add_reward(&db, 1, 100 + i, (i + 1) as i32).await.expect("add");
            assert_eq!(outcome, AddOutcome::Added, "reward {} must fit", i + 1);
        }
        let outcome = add_reward(&db, 1, 999, 999).await.expect("21st add");
        assert_eq!(outcome, AddOutcome::LimitReached, "the 21st reward is blocked");
        assert_eq!(list_rewards(&db, 1).await.expect("list").len(), MAX_REWARDS);
    }

    #[tokio::test]
    async fn rewards_are_scoped_per_guild() {
        let db = fresh_db().await;
        add_reward(&db, 1, 42, 100).await.expect("add");
        // Same role and requirement in another guild is fine.
        let outcome = add_reward(&db, 2, 42, 100).await.expect("add other guild");
        assert_eq!(outcome, AddOutcome::Added);
        assert_eq!(list_rewards(&db, 1).await.expect("list").len(), 1);
        assert_eq!(list_rewards(&db, 2).await.expect("list").len(), 1);
    }

    #[tokio::test]
    async fn f24_remove_reward_works_by_stored_id_without_any_role_cache() {
        let db = fresh_db().await;
        add_reward(&db, 1, 42, 100).await.expect("add");
        // The Discord role may not exist anymore — removal only needs the id.
        let outcome = remove_reward(&db, 1, 42).await.expect("remove");
        assert_eq!(outcome, RemoveOutcome::Removed);
        assert!(list_rewards(&db, 1).await.expect("list").is_empty());
    }

    #[tokio::test]
    async fn remove_reward_distinguishes_no_rewards_from_not_a_reward() {
        let db = fresh_db().await;
        assert_eq!(
            remove_reward(&db, 1, 42).await.expect("remove"),
            RemoveOutcome::NoRewards,
            "empty guild"
        );
        add_reward(&db, 1, 42, 100).await.expect("add");
        assert_eq!(
            remove_reward(&db, 1, 43).await.expect("remove"),
            RemoveOutcome::NotAReward,
            "other role"
        );
        assert_eq!(list_rewards(&db, 1).await.expect("list").len(), 1, "nothing deleted");
    }

    #[tokio::test]
    async fn clear_rewards_deletes_everything_and_errors_when_empty() {
        let db = fresh_db().await;
        assert_eq!(clear_rewards(&db, 1).await.expect("clear"), ClearOutcome::NoRewards);

        add_reward(&db, 1, 42, 100).await.expect("add");
        add_reward(&db, 1, 43, 200).await.expect("add");
        add_reward(&db, 2, 44, 300).await.expect("add other guild");

        assert_eq!(clear_rewards(&db, 1).await.expect("clear"), ClearOutcome::Cleared(2));
        assert!(list_rewards(&db, 1).await.expect("list").is_empty());
        assert_eq!(
            list_rewards(&db, 2).await.expect("list").len(),
            1,
            "other guilds untouched"
        );
    }

    #[tokio::test]
    async fn list_rewards_sorts_by_requirement_descending() {
        let db = fresh_db().await;
        add_reward(&db, 1, 10, 50).await.expect("add");
        add_reward(&db, 1, 20, 200).await.expect("add");
        add_reward(&db, 1, 30, 100).await.expect("add");

        let rewards = list_rewards(&db, 1).await.expect("list");
        let pairs: Vec<(i64, i32)> = rewards.iter().map(|r| (r.role_id, r.requirement)).collect();
        assert_eq!(pairs, vec![(20, 200), (30, 100), (10, 50)]);
    }
}
