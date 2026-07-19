//! `/panel create|delete` — the single persistent leaderboard message
//! (batch C13).
//!
//! Spec: docs/specs/commands-admin.md §/panel. Fixes applied:
//! - F30: `create` best-effort deletes the previous panel message after the
//!   replacement was sent (legacy orphaned it forever) — send-new-first so a
//!   failed render/send never destroys a working panel; `delete` also
//!   deletes the Discord message, not just the record.
//! - F31: the DB record is written ONLY after the panel message was
//!   successfully sent — a failed send leaves no record (legacy persisted a
//!   record pointing at a raw "Creating panel..." stub).
//! - The legacy "Creating panel..." two-step is gone: the message is sent
//!   already carrying the rendered leaderboard embed.
//! - QUIRK fix: `delete` with no panel says so instead of claiming success.
//!
//! Deviation from legacy (justified): legacy publicly deferred and then
//! DELETED its own interaction reply on success. The intent — no visible
//! chatter next to the panel — is served cleaner by an ephemeral
//! confirmation, matching the Rust-target style used by `/export`.
//!
//! Panel CONTENT renders with the default locale (`i18n::resolve(None)`),
//! not the invoker's: the background updater has no interaction locale, and
//! the message must not flip language between refreshes. Confirmations use
//! the invoker's locale as usual. Rendering shares `crate::bot::render`
//! with `/leaderboard` (page 1, no footer — legacy panels had no footer).
//!
//! Refreshes are event-driven via `crate::bot::panel_updater` (F29/F32).

use poise::serenity_prelude as serenity;

use crate::bot::{medals_panel, panel_updater, render, util, Context, Error};
use crate::domain::guild_links;
use crate::i18n;

/// Create a leaderboard panel. You can only have one panel at a time.
#[poise::command(
    slash_command,
    guild_only,
    default_member_permissions = "MANAGE_GUILD",
    required_permissions = "MANAGE_GUILD",
    subcommands("create", "delete", "medals", "overview"),
    subcommand_required
)]
pub async fn panel(_ctx: Context<'_>) -> Result<(), Error> {
    // Never reached: subcommand_required.
    Ok(())
}

/// Manage active-medals catalog panels, one per category.
#[poise::command(
    slash_command,
    guild_only,
    subcommands("medals_create", "medals_delete"),
    subcommand_required,
    rename = "medals"
)]
async fn medals(_ctx: Context<'_>) -> Result<(), Error> {
    // Never reached: subcommand_required.
    Ok(())
}

/// Manage the all-categories medals overview panel.
#[poise::command(
    slash_command,
    guild_only,
    subcommands("overview_create", "overview_delete"),
    subcommand_required,
    rename = "overview"
)]
async fn overview(_ctx: Context<'_>) -> Result<(), Error> {
    // Never reached: subcommand_required.
    Ok(())
}

/// Create the panel for the leaderboard.
#[poise::command(slash_command, guild_only)]
async fn create(ctx: Context<'_>) -> Result<(), Error> {
    let locale = util::locale(&ctx);
    let guild_id = util::require_guild_id(&ctx)?;
    let db = &ctx.data().db;

    // The first render can exceed the 3 s interaction window (member
    // lookups), so acknowledge early; the confirmation stays ephemeral.
    ctx.defer_ephemeral().await?;

    // Cross-guild link (guild_links): if this guild mirrors another one,
    // the panel renders (and later refreshes) THAT guild's leaderboard.
    let source = guild_links::accepted_source_for(db, guild_id.get() as i64).await?;
    let data_guild_id = source.map_or(guild_id, |id| serenity::GuildId::new(id as u64));

    let panel_locale = i18n::resolve(None);
    let guild_name =
        panel_updater::guild_display_name(ctx.serenity_context(), data_guild_id, &panel_locale)
            .await;
    let embed = render::render_leaderboard(
        db,
        ctx.serenity_context(),
        data_guild_id,
        &guild_name,
        1,
        &panel_locale,
        false,
    )
    .await?;

    // F31: send first; the record exists only for a message that exists.
    // The new message is sent BEFORE the old panel is touched, so a failed
    // render/send leaves any existing panel fully intact (message + record).
    let message = match ctx
        .channel_id()
        .send_message(ctx.serenity_context(), serenity::CreateMessage::new().embed(embed))
        .await
    {
        Ok(message) => message,
        Err(error) => {
            log::warn!(
                "/panel create could not send the panel message (guild={}, channel={}): {error}",
                guild_id.get(),
                ctx.channel_id().get()
            );
            return util::reply_error(ctx, i18n::t(&locale, "panel-create-failed"), true).await;
        }
    };

    // F30: replace semantics — drop the previous panel message (best
    // effort, logged inside) so it is not orphaned in its channel. Done
    // only after the replacement exists.
    if let Some(old) = panel_updater::get_panel(db, guild_id.get() as i64).await? {
        panel_updater::delete_panel_message(ctx.serenity_context(), &old).await;
    }

    panel_updater::save_panel(
        db,
        guild_id.get() as i64,
        ctx.channel_id().get() as i64,
        message.id.get() as i64,
        source,
    )
    .await?;

    let embed = serenity::CreateEmbed::new()
        .colour(util::COLOR_SUCCESS)
        .description(i18n::t(&locale, "panel-created"));
    util::reply_embed(ctx, embed, true).await
}

/// Delete the panel for the leaderboard.
#[poise::command(slash_command, guild_only)]
async fn delete(ctx: Context<'_>) -> Result<(), Error> {
    let locale = util::locale(&ctx);
    let guild_id = util::require_guild_id(&ctx)?;
    let db = &ctx.data().db;

    let Some(panel) = panel_updater::get_panel(db, guild_id.get() as i64).await? else {
        // QUIRK fix: legacy reported success even when nothing existed.
        return util::reply_error(ctx, i18n::t(&locale, "panel-delete-none"), true).await;
    };

    // F30: remove the Discord message too (best effort, logged inside).
    panel_updater::delete_panel_message(ctx.serenity_context(), &panel).await;
    panel_updater::remove_panel(db, guild_id.get() as i64).await?;

    let embed = serenity::CreateEmbed::new()
        .colour(util::COLOR_SUCCESS)
        .description(i18n::t(&locale, "panel-deleted"));
    util::reply_embed(ctx, embed, true).await
}

/// Create a catalog panel of active medals for one category.
#[poise::command(
    slash_command,
    guild_only,
    default_member_permissions = "MANAGE_GUILD",
    required_permissions = "MANAGE_GUILD",
    rename = "create"
)]
async fn medals_create(
    ctx: Context<'_>,
    #[description = "The category to build a catalog panel for"]
    #[autocomplete = "medals_panel::autocomplete_category"]
    category: String,
) -> Result<(), Error> {
    let locale = util::locale(&ctx);
    let guild_id = util::require_guild_id(&ctx)?;
    let db = &ctx.data().db;

    // The first render queries the DB and can exceed the 3s window on a
    // large category — acknowledge early; the confirmation stays ephemeral.
    ctx.defer_ephemeral().await?;

    // Cross-guild link: if this guild mirrors another one, the panel
    // renders (and later refreshes) THAT guild's category catalog.
    let source = guild_links::accepted_source_for(db, guild_id.get() as i64).await?;
    let data_guild_id = source.unwrap_or(guild_id.get() as i64);

    let panel_locale = i18n::resolve(None);
    let embed =
        medals_panel::render_category_embed(db, data_guild_id, &category, &panel_locale).await?;

    // F31-style: send first; the record exists only for a message that exists.
    let message = match ctx
        .channel_id()
        .send_message(ctx.serenity_context(), serenity::CreateMessage::new().embed(embed))
        .await
    {
        Ok(message) => message,
        Err(error) => {
            log::warn!(
                "/panel medals create could not send the panel message (guild={}, channel={}): {error}",
                guild_id.get(),
                ctx.channel_id().get()
            );
            return util::reply_error(ctx, i18n::t(&locale, "panel-medals-create-failed"), true)
                .await;
        }
    };

    // F30-style replace semantics: drop the previous panel for this
    // category (best effort, logged inside), only after the replacement exists.
    if let Some(old) = medals_panel::get_panel(db, guild_id.get() as i64, &category).await? {
        medals_panel::delete_panel_message(ctx.serenity_context(), &old).await;
    }

    medals_panel::save_panel(
        db,
        guild_id.get() as i64,
        &category,
        ctx.channel_id().get() as i64,
        message.id.get() as i64,
        source,
    )
    .await?;

    let embed = serenity::CreateEmbed::new()
        .colour(util::COLOR_SUCCESS)
        .description(i18n::t_args(
            &locale,
            "panel-medals-created",
            &[("category", category.into())],
        ));
    util::reply_embed(ctx, embed, true).await
}

/// Delete the catalog panel for one category.
#[poise::command(
    slash_command,
    guild_only,
    default_member_permissions = "MANAGE_GUILD",
    required_permissions = "MANAGE_GUILD",
    rename = "delete"
)]
async fn medals_delete(
    ctx: Context<'_>,
    #[description = "The category whose catalog panel should be removed"]
    #[autocomplete = "medals_panel::autocomplete_category"]
    category: String,
) -> Result<(), Error> {
    let locale = util::locale(&ctx);
    let guild_id = util::require_guild_id(&ctx)?;
    let db = &ctx.data().db;

    let Some(panel) = medals_panel::get_panel(db, guild_id.get() as i64, &category).await? else {
        return util::reply_error(ctx, i18n::t(&locale, "panel-medals-delete-none"), true).await;
    };

    medals_panel::delete_panel_message(ctx.serenity_context(), &panel).await;
    medals_panel::remove_panel(db, guild_id.get() as i64, &category).await?;

    let embed = serenity::CreateEmbed::new()
        .colour(util::COLOR_SUCCESS)
        .description(i18n::t_args(
            &locale,
            "panel-medals-deleted",
            &[("category", category.into())],
        ));
    util::reply_embed(ctx, embed, true).await
}

/// Create a catalog panel of every active medal, sectioned by category.
#[poise::command(
    slash_command,
    guild_only,
    default_member_permissions = "MANAGE_GUILD",
    required_permissions = "MANAGE_GUILD",
    rename = "create"
)]
async fn overview_create(ctx: Context<'_>) -> Result<(), Error> {
    let locale = util::locale(&ctx);
    let guild_id = util::require_guild_id(&ctx)?;
    let db = &ctx.data().db;

    // The first render queries the DB and can exceed the 3s window on a
    // large catalog — acknowledge early; the confirmation stays ephemeral.
    ctx.defer_ephemeral().await?;

    // Cross-guild link: if this guild mirrors another one, the panel
    // renders (and later refreshes) THAT guild's full catalog.
    let source = guild_links::accepted_source_for(db, guild_id.get() as i64).await?;
    let data_guild_id = source.unwrap_or(guild_id.get() as i64);

    let panel_locale = i18n::resolve(None);
    let embed = medals_panel::render_overview_embed(db, data_guild_id, &panel_locale).await?;

    // F31-style: send first; the record exists only for a message that exists.
    let message = match ctx
        .channel_id()
        .send_message(ctx.serenity_context(), serenity::CreateMessage::new().embed(embed))
        .await
    {
        Ok(message) => message,
        Err(error) => {
            log::warn!(
                "/panel overview create could not send the panel message (guild={}, channel={}): {error}",
                guild_id.get(),
                ctx.channel_id().get()
            );
            return util::reply_error(ctx, i18n::t(&locale, "panel-overview-create-failed"), true)
                .await;
        }
    };

    // F30-style replace semantics: drop the previous overview panel (best
    // effort, logged inside), only after the replacement exists.
    if let Some(old) = medals_panel::get_overview_panel(db, guild_id.get() as i64).await? {
        medals_panel::delete_overview_panel_message(ctx.serenity_context(), &old).await;
    }

    medals_panel::save_overview_panel(
        db,
        guild_id.get() as i64,
        ctx.channel_id().get() as i64,
        message.id.get() as i64,
        source,
    )
    .await?;

    let embed = serenity::CreateEmbed::new()
        .colour(util::COLOR_SUCCESS)
        .description(i18n::t(&locale, "panel-overview-created"));
    util::reply_embed(ctx, embed, true).await
}

/// Delete the all-categories medals overview panel.
#[poise::command(
    slash_command,
    guild_only,
    default_member_permissions = "MANAGE_GUILD",
    required_permissions = "MANAGE_GUILD",
    rename = "delete"
)]
async fn overview_delete(ctx: Context<'_>) -> Result<(), Error> {
    let locale = util::locale(&ctx);
    let guild_id = util::require_guild_id(&ctx)?;
    let db = &ctx.data().db;

    let Some(panel) = medals_panel::get_overview_panel(db, guild_id.get() as i64).await? else {
        return util::reply_error(ctx, i18n::t(&locale, "panel-overview-delete-none"), true).await;
    };

    medals_panel::delete_overview_panel_message(ctx.serenity_context(), &panel).await;
    medals_panel::remove_overview_panel(db, guild_id.get() as i64).await?;

    let embed = serenity::CreateEmbed::new()
        .colour(util::COLOR_SUCCESS)
        .description(i18n::t(&locale, "panel-overview-deleted"));
    util::reply_embed(ctx, embed, true).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registers_create_delete_medals_and_overview_subcommands_and_requires_one() {
        let command = panel();
        assert_eq!(command.name, "panel");
        assert!(command.subcommand_required);
        for name in ["create", "delete", "medals", "overview"] {
            assert!(
                command.subcommands.iter().any(|sub| sub.name == name),
                "missing subcommand {name}"
            );
        }
        assert_eq!(command.subcommands.len(), 4);
    }

    #[test]
    fn medals_group_registers_create_and_delete_and_requires_one() {
        let command = panel();
        let medals = command
            .subcommands
            .iter()
            .find(|sub| sub.name == "medals")
            .expect("medals subcommand group registered");
        assert!(medals.subcommand_required);
        for name in ["create", "delete"] {
            assert!(
                medals.subcommands.iter().any(|sub| sub.name == name),
                "missing /panel medals {name}"
            );
        }
        assert_eq!(medals.subcommands.len(), 2);
    }

    #[test]
    fn overview_group_registers_create_and_delete_and_requires_one() {
        let command = panel();
        let overview = command
            .subcommands
            .iter()
            .find(|sub| sub.name == "overview")
            .expect("overview subcommand group registered");
        assert!(overview.subcommand_required);
        for name in ["create", "delete"] {
            assert!(
                overview.subcommands.iter().any(|sub| sub.name == name),
                "missing /panel overview {name}"
            );
        }
        assert_eq!(overview.subcommands.len(), 2);
    }

    #[test]
    fn subcommand_descriptions_match_the_legacy_texts() {
        let command = panel();
        let description = |name: &str| {
            command
                .subcommands
                .iter()
                .find(|sub| sub.name == name)
                .and_then(|sub| sub.description.clone())
                .unwrap_or_default()
        };
        assert_eq!(description("create"), "Create the panel for the leaderboard.");
        assert_eq!(description("delete"), "Delete the panel for the leaderboard.");
    }

    #[test]
    fn catalog_keys_exist() {
        let locale = i18n::resolve(None);
        for key in [
            "panel-created",
            "panel-create-failed",
            "panel-deleted",
            "panel-delete-none",
        ] {
            assert_ne!(i18n::t(&locale, key), key, "missing catalog key {key}");
        }
    }

    #[test]
    fn medals_catalog_keys_exist_and_interpolate_the_category() {
        let locale = i18n::resolve(None);
        let args: &[(&'static str, i18n::FluentValue<'static>)] =
            &[("category", "Government".into())];
        for key in ["panel-medals-created", "panel-medals-deleted", "panel-medals-delete-none"] {
            let message = i18n::t_args(&locale, key, args);
            assert_ne!(message, key, "missing catalog key {key}");
            assert!(message.contains("Government"), "{key} got: {message}");
        }
        assert_ne!(
            i18n::t(&locale, "panel-medals-create-failed"),
            "panel-medals-create-failed"
        );
    }

    #[test]
    fn overview_catalog_keys_exist() {
        let locale = i18n::resolve(None);
        for key in [
            "panel-overview-created",
            "panel-overview-create-failed",
            "panel-overview-deleted",
            "panel-overview-delete-none",
        ] {
            assert_ne!(i18n::t(&locale, key), key, "missing catalog key {key}");
        }
    }
}
