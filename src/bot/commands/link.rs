//! `/link` — full cross-guild co-administration (schema.md `guild_links`).
//!
//! Mutual-consent link between two guilds: once the SOURCE guild ("A")
//! accepts a request from the LINKED guild ("B"), B becomes a full second
//! control room for A. Every trophy-content command run in B
//! (`/create`, `/edit`, `/delete`, `/award`, `/revoke`, `/clear`, `/details`,
//! `/show`, `/trophies`, `/leaderboard`, plus both panel types) transparently
//! reads and writes A's data instead of B's own — see
//! `util::effective_guild_id`, the single primitive every one of those
//! commands routes through. `MANAGE_GUILD` in B is still what gates who may
//! run them; A is trusting B's admins with the same reach as its own.
//!
//! Business logic (parsing, the actual reads/writes) lives in
//! `crate::domain::guild_links`; this file is the thin poise layer plus the
//! panel cleanup that revoke triggers.

use poise::serenity_prelude as serenity;

use crate::bot::{medals_panel, panel_updater, util, Context, Error};
use crate::domain::guild_links;
use crate::i18n;

/// Discord guild IDs are snowflakes: digits only, 15-20 of them (the same
/// bound `create::parse_dedication` uses for user snowflakes).
pub(crate) fn parse_guild_id(raw: &str) -> Option<i64> {
    let trimmed = raw.trim();
    let in_range = (15..=20).contains(&trimmed.len());
    if in_range && trimmed.bytes().all(|b| b.is_ascii_digit()) {
        trimmed.parse::<u64>().ok().map(|id| id as i64)
    } else {
        None
    }
}

/// Best-effort teardown of every panel `linked_guild_id` had pointed at
/// `source_guild_id` (both panel types) — run right after a link is
/// severed so the linked guild is never left displaying data it no longer
/// has permission to see. Failures are logged, not propagated: the link
/// itself is already gone by the time this runs, and the render-time
/// re-validation in both updaters is the actual safety net.
async fn cleanup_linked_panels(
    ctx: &serenity::Context,
    db: &sea_orm::DatabaseConnection,
    source_guild_id: i64,
    linked_guild_id: i64,
) {
    if let Ok(Some(panel)) = panel_updater::get_panel(db, linked_guild_id).await
        && panel.source_guild_id == Some(source_guild_id)
    {
        panel_updater::delete_panel_message(ctx, &panel).await;
        if let Err(error) = panel_updater::remove_panel(db, linked_guild_id).await {
            log::error!("Failed to remove leaderboard panel after link revoke: {error:#}");
        }
    }

    match medals_panel::panels_from_source(db, linked_guild_id, source_guild_id).await {
        Ok(panels) => {
            for panel in &panels {
                medals_panel::delete_panel_message(ctx, panel).await;
                if let Err(error) = medals_panel::remove_panel(db, linked_guild_id, &panel.category).await {
                    log::error!("Failed to remove medals panel after link revoke: {error:#}");
                }
            }
        }
        Err(error) => log::error!("Failed to list medals panels to clean up after link revoke: {error:#}"),
    }
}

/// Link this server's panels to another server's, or manage an existing link.
#[poise::command(
    slash_command,
    guild_only,
    default_member_permissions = "MANAGE_GUILD",
    required_permissions = "MANAGE_GUILD",
    subcommands("request", "accept", "revoke", "status"),
    subcommand_required
)]
pub async fn link(_ctx: Context<'_>) -> Result<(), Error> {
    // Never reached: subcommand_required.
    Ok(())
}

/// Ask another server to let this server's panels mirror its medals.
#[poise::command(slash_command, guild_only)]
async fn request(
    ctx: Context<'_>,
    #[description = "The ID of the server whose medals you want to display"] guild_id: String,
) -> Result<(), Error> {
    let locale = util::locale(&ctx);
    let own_guild_id = util::require_guild_id(&ctx)?.get() as i64;
    let db = &ctx.data().db;

    let Some(target) = parse_guild_id(&guild_id) else {
        return util::reply_error(ctx, i18n::t(&locale, "link-error-invalid-guild-id"), true).await;
    };

    ctx.defer_ephemeral().await?;

    // Confirm the bot is actually IN that guild before creating a request
    // pointing at it — a typo'd ID must not create a dangling row.
    if serenity::GuildId::new(target as u64)
        .to_partial_guild(ctx.serenity_context())
        .await
        .is_err()
    {
        return util::reply_error_ephemeral(ctx, i18n::t(&locale, "link-error-guild-not-found")).await;
    }

    let author = ctx.author().id.get() as i64;
    match guild_links::request_link(db, target, own_guild_id, author).await? {
        Ok(()) => {
            let embed = serenity::CreateEmbed::new()
                .colour(util::COLOR_SUCCESS)
                .description(i18n::t(&locale, "link-request-sent"));
            util::reply_embed(ctx, embed, true).await
        }
        Err(guild_links::RequestError::SelfLink) => {
            util::reply_error_ephemeral(ctx, i18n::t(&locale, "link-error-self-link")).await
        }
        Err(guild_links::RequestError::AlreadyLinked) => {
            util::reply_error_ephemeral(ctx, i18n::t(&locale, "link-error-already-linked")).await
        }
    }
}

/// Poise autocomplete: pending requesters for THIS guild (as a source) only
/// — never a global guild listing, so no server is exposed to another.
async fn autocomplete_pending(ctx: Context<'_>, _partial: &str) -> Vec<String> {
    let Some(guild_id) = ctx.guild_id() else {
        return Vec::new();
    };
    guild_links::pending_requesters(&ctx.data().db, guild_id.get() as i64)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|id| id.to_string())
        .collect()
}

/// Poise autocomplete: guilds currently linked (accepted) into THIS guild's
/// panels — used by `/link revoke` on the source side.
async fn autocomplete_linked(ctx: Context<'_>, _partial: &str) -> Vec<String> {
    let Some(guild_id) = ctx.guild_id() else {
        return Vec::new();
    };
    guild_links::linked_guilds(&ctx.data().db, guild_id.get() as i64)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|id| id.to_string())
        .collect()
}

/// Accept a pending request from another server.
#[poise::command(
    slash_command,
    guild_only,
    default_member_permissions = "MANAGE_GUILD",
    required_permissions = "MANAGE_GUILD"
)]
async fn accept(
    ctx: Context<'_>,
    #[description = "The server whose request you're accepting"]
    #[autocomplete = "autocomplete_pending"]
    guild_id: String,
) -> Result<(), Error> {
    let locale = util::locale(&ctx);
    let own_guild_id = util::require_guild_id(&ctx)?.get() as i64;
    let db = &ctx.data().db;

    let Some(target) = parse_guild_id(&guild_id) else {
        return util::reply_error(ctx, i18n::t(&locale, "link-error-invalid-guild-id"), true).await;
    };

    let author = ctx.author().id.get() as i64;
    match guild_links::accept_link(db, own_guild_id, target, author).await? {
        Ok(()) => {
            let embed = serenity::CreateEmbed::new()
                .colour(util::COLOR_SUCCESS)
                .description(i18n::t(&locale, "link-accepted"));
            util::reply_embed(ctx, embed, true).await
        }
        Err(guild_links::AcceptError::NoSuchRequest) => {
            util::reply_error(ctx, i18n::t(&locale, "link-error-no-such-request"), true).await
        }
    }
}

/// Remove a link between this server and another.
#[poise::command(
    slash_command,
    guild_only,
    default_member_permissions = "MANAGE_GUILD",
    required_permissions = "MANAGE_GUILD"
)]
async fn revoke(
    ctx: Context<'_>,
    #[description = "The server to unlink (only needed on the source side)"]
    #[autocomplete = "autocomplete_linked"]
    guild_id: Option<String>,
) -> Result<(), Error> {
    let locale = util::locale(&ctx);
    let own_guild_id = util::require_guild_id(&ctx)?.get() as i64;
    let db = &ctx.data().db;

    let (source_guild_id, linked_guild_id) = match guild_id {
        Some(raw) => {
            let Some(target) = parse_guild_id(&raw) else {
                return util::reply_error(ctx, i18n::t(&locale, "link-error-invalid-guild-id"), true)
                    .await;
            };
            (own_guild_id, target)
        }
        None => match guild_links::link_as_linked_guild(db, own_guild_id).await? {
            Some(row) => (row.source_guild_id, own_guild_id),
            None => {
                return util::reply_error(ctx, i18n::t(&locale, "link-error-nothing-to-revoke"), true)
                    .await;
            }
        },
    };

    if !guild_links::revoke_link(db, source_guild_id, linked_guild_id).await? {
        return util::reply_error(ctx, i18n::t(&locale, "link-error-nothing-to-revoke"), true).await;
    }

    cleanup_linked_panels(ctx.serenity_context(), db, source_guild_id, linked_guild_id).await;

    let embed = serenity::CreateEmbed::new()
        .colour(util::COLOR_SUCCESS)
        .description(i18n::t(&locale, "link-revoked"));
    util::reply_embed(ctx, embed, true).await
}

/// Show this server's current link status.
#[poise::command(slash_command, guild_only)]
async fn status(ctx: Context<'_>) -> Result<(), Error> {
    let locale = util::locale(&ctx);
    let own_guild_id = util::require_guild_id(&ctx)?.get() as i64;
    let db = &ctx.data().db;

    let mut lines = Vec::new();
    if let Some(row) = guild_links::link_as_linked_guild(db, own_guild_id).await? {
        let key = if row.accepted_at.is_some() { "link-status-linked-to" } else { "link-status-pending-to" };
        lines.push(i18n::t_args(&locale, key, &[("guild", row.source_guild_id.to_string().into())]));
    }
    for source in guild_links::linked_guilds(db, own_guild_id).await? {
        lines.push(i18n::t_args(
            &locale,
            "link-status-linked-from",
            &[("guild", source.to_string().into())],
        ));
    }
    for pending in guild_links::pending_requesters(db, own_guild_id).await? {
        lines.push(i18n::t_args(
            &locale,
            "link-status-pending-from",
            &[("guild", pending.to_string().into())],
        ));
    }
    if lines.is_empty() {
        lines.push(i18n::t(&locale, "link-status-none"));
    }

    let embed = serenity::CreateEmbed::new()
        .colour(util::COLOR_MAIN)
        .description(lines.join("\n"));
    util::reply_embed(ctx, embed, true).await
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- parse_guild_id ---

    #[test]
    fn valid_snowflakes_parse() {
        assert_eq!(parse_guild_id("123456789012345678"), Some(123456789012345678));
        assert_eq!(parse_guild_id("  123456789012345678  "), Some(123456789012345678));
        assert_eq!(parse_guild_id("100000000000000"), Some(100000000000000)); // 15 digits
    }

    #[test]
    fn malformed_input_is_rejected() {
        for raw in ["", "not-a-guild", "123", "abc123456789012345", "<@123456789012345678>"] {
            assert_eq!(parse_guild_id(raw), None, "input: {raw:?}");
        }
    }

    // --- command registration ---

    #[test]
    fn registers_request_accept_revoke_status_and_requires_one() {
        let command = link();
        assert_eq!(command.name, "link");
        assert!(command.subcommand_required);
        for name in ["request", "accept", "revoke", "status"] {
            assert!(
                command.subcommands.iter().any(|sub| sub.name == name),
                "missing subcommand {name}"
            );
        }
        assert_eq!(command.subcommands.len(), 4);
    }

    // --- i18n catalog ---

    #[test]
    fn catalog_keys_exist() {
        let locale = i18n::resolve(None);
        for key in [
            "link-error-invalid-guild-id",
            "link-error-guild-not-found",
            "link-request-sent",
            "link-error-self-link",
            "link-error-already-linked",
            "link-accepted",
            "link-error-no-such-request",
            "link-error-nothing-to-revoke",
            "link-revoked",
            "link-status-none",
        ] {
            assert_ne!(i18n::t(&locale, key), key, "missing catalog key {key}");
        }
        let args: &[(&'static str, i18n::FluentValue<'static>)] = &[("guild", "42".into())];
        for key in [
            "link-status-linked-to",
            "link-status-pending-to",
            "link-status-linked-from",
            "link-status-pending-from",
        ] {
            let message = i18n::t_args(&locale, key, args);
            assert_ne!(message, key, "missing catalog key {key}");
            assert!(message.contains("42"), "{key} got: {message}");
        }
    }
}
