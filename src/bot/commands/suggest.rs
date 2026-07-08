//! `/suggest` — static redirect to the support server
//! (spec: docs/specs/commands-utility.md; the in-bot suggestion system was
//! removed in legacy v1.4).
//!
//! Fix vs legacy: the declared 10s cooldown is actually enforced
//! (Poise `user_cooldown`; the legacy dispatcher never read it).

use poise::serenity_prelude as serenity;

use crate::bot::{Context, Error, util};
use crate::i18n;

/// Suggest a feature or change for the bot
#[poise::command(slash_command, user_cooldown = 10)]
pub async fn suggest(ctx: Context<'_>) -> Result<(), Error> {
    let locale = util::locale(&ctx);

    let embed = serenity::CreateEmbed::new()
        .title(i18n::t(&locale, "suggest-title"))
        .description(i18n::t(&locale, "suggest-description"))
        .colour(util::COLOR_MAIN);

    util::reply_embed(ctx, embed, false).await
}

#[cfg(test)]
mod tests {
    use crate::i18n;

    #[test]
    fn suggest_catalog_redirects_to_support_server() {
        let locale = i18n::resolve(None);
        assert_ne!(i18n::t(&locale, "suggest-title"), "suggest-title");
        let description = i18n::t(&locale, "suggest-description");
        assert!(description.contains("discord.gg/kNmgU44xgU"), "got: {description}");
    }
}
