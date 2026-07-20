//! `/help` — static usage guide (spec: docs/specs/commands-utility.md).
//!
//! Content is REWRITTEN per rust-parity-plan.md: it describes the real,
//! current command set and Discord-native permissions only — the legacy
//! help text taught the deprecated custom `/permissions` system.

use poise::serenity_prelude as serenity;

use crate::bot::{Context, Error, util};
use crate::i18n;

/// Stop it! Get some help!
#[poise::command(slash_command)]
pub async fn help(ctx: Context<'_>) -> Result<(), Error> {
    let locale = util::locale(&ctx);

    let embed = serenity::CreateEmbed::new()
        .title(i18n::t(&locale, "help-title"))
        .description(i18n::t(&locale, "help-description"))
        .colour(util::COLOR_MAIN);

    util::reply_embed(ctx, embed, false).await
}

#[cfg(test)]
mod tests {
    use crate::i18n;

    #[test]
    fn help_catalog_exists() {
        let locale = i18n::resolve(None);
        assert_ne!(i18n::t(&locale, "help-title"), "help-title");
        assert_ne!(i18n::t(&locale, "help-description"), "help-description");
    }

    #[test]
    fn help_lists_current_commands() {
        let locale = i18n::resolve(None);
        let description = i18n::t(&locale, "help-description");
        for command in [
            "/create",
            "/edit",
            "/delete",
            "/award",
            "/revoke",
            "/clear",
            "/details",
            "/show",
            "/trophies",
            "/leaderboard",
            "/settings",
            "/rewards",
            "/panel",
            "/panel medals",
            "/panel overview",
            "/panel retired",
            "/link",
            "/export",
        ] {
            assert!(description.contains(command), "missing {command}");
        }
        // Embeds cap descriptions at 4096 characters.
        assert!(description.chars().count() <= 4096);
    }

    #[test]
    fn help_does_not_teach_the_deprecated_permission_system() {
        let locale = i18n::resolve(None);
        let description = i18n::t(&locale, "help-description");
        assert!(
            !description.contains("/permissions"),
            "help must not reference the deprecated /permissions command"
        );
        assert!(
            description.contains("Integrations"),
            "help must point at Discord-native permissions (Server Settings → Integrations)"
        );
    }
}
