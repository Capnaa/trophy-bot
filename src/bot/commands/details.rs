//! `/details` — show a trophy's private details text (batch C10).
//!
//! Spec: docs/specs/commands-trophy-management.md §/details. Fixes applied:
//! - F11: the reply is EPHEMERAL (legacy posted the "private" details in a
//!   public message) and Manage Guild is enforced like the rest of the
//!   management set (`default_member_permissions` below).
//! - F12: trophy resolved by exact normalized name with autocomplete via the
//!   shared resolver (`src/bot/resolver.rs`) — no numeric-ID branch, no
//!   substring matching, and the "Trophy ID" footer becomes the trophy name
//!   (UUIDs are never user-facing).
//!
//! The legacy easter-egg embed URL (details.js:36-42) is kept deliberately,
//! matching the choice made for `/show`.
//!
//! Business logic lives in plain testable functions; the handler stays thin.

use poise::serenity_prelude as serenity;

use crate::bot::{Context, Error, resolver, util};
use crate::i18n;

/// Legacy easter-egg embed URL, kept deliberately for parity (details.js:40).
const EMBED_URL: &str = "https://www.youtube.com/watch?v=PwP9ebvCBAM";

/// Shows the details of a trophy.
#[poise::command(slash_command, guild_only, default_member_permissions = "MANAGE_GUILD", required_permissions = "MANAGE_GUILD")]
pub async fn details(
    ctx: Context<'_>,
    #[description = "Name of the trophy to show"]
    #[autocomplete = "resolver::autocomplete_trophy"]
    trophy: String,
) -> Result<(), Error> {
    let locale = util::locale(&ctx);
    let guild_id = util::require_guild_id(&ctx)?;

    let Some(model) = resolver::resolve_trophy_or_reply(
        ctx,
        guild_id.get() as i64,
        &trophy,
        "details-error-not-found",
    )
    .await?
    else {
        return Ok(());
    };

    let embed = serenity::CreateEmbed::new()
        .title(format!("{} {}", model.emoji, model.name))
        .url(EMBED_URL)
        .description(model.details.clone())
        .colour(util::COLOR_MAIN)
        .footer(serenity::CreateEmbedFooter::new(i18n::t_args(
            &locale,
            "details-footer",
            &[("name", model.name.clone().into())],
        )));

    // F11: private details stay private.
    util::reply_embed(ctx, embed, true).await
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- i18n catalog ---

    #[test]
    fn all_details_messages_exist() {
        let locale = i18n::resolve(None);
        let args: &[(&'static str, i18n::FluentValue<'static>)] =
            &[("input", "Gold".into()), ("name", "Gold".into())];
        for key in ["details-error-not-found", "details-footer"] {
            assert_ne!(i18n::t_args(&locale, key, args), key, "missing ftl message: {key}");
        }
    }

    #[test]
    fn not_found_message_renders_the_input() {
        let locale = i18n::resolve(None);
        let message =
            i18n::t_args(&locale, "details-error-not-found", &[("input", "Golde".into())]);
        assert!(message.contains("Golde"), "got: {message}");
    }
}
