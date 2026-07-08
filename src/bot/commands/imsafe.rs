//! `/imsafe` — kept as a no-op confirmation for continuity
//! (spec: docs/specs/commands-utility.md "Rust target" + rust-parity-plan.md §1).
//!
//! The legacy imsafe gate is retired: management commands rely on
//! Discord-native permissions only, so every guild is always "safe".
//! The command performs no data operations and always replies with the
//! legacy "already safe" confirmation.

use poise::serenity_prelude as serenity;

use crate::bot::{Context, Error, util};
use crate::i18n;

/// Confirms you're using Discord permissions instead of the deprecated custom permissions
#[poise::command(slash_command, guild_only, default_member_permissions = "MANAGE_GUILD", required_permissions = "MANAGE_GUILD")]
pub async fn imsafe(ctx: Context<'_>) -> Result<(), Error> {
    let locale = util::locale(&ctx);

    let embed = serenity::CreateEmbed::new()
        .description(i18n::t(&locale, "imsafe-safe"))
        .colour(util::COLOR_MAIN);

    util::reply_embed(ctx, embed, false).await
}

#[cfg(test)]
mod tests {
    use crate::i18n;

    #[test]
    fn imsafe_catalog_confirms_safe_mode() {
        let locale = i18n::resolve(None);
        let message = i18n::t(&locale, "imsafe-safe");
        assert_ne!(message, "imsafe-safe");
        assert!(message.contains("safe mode"), "got: {message}");
    }
}
