//! `/about` — static informational embed (spec: docs/specs/commands-utility.md).
//!
//! Links to GitHub, Ko-fi and the support server, credits the creator and
//! shows the version pulled from `Cargo.toml` (per the spec's Rust target).

use poise::serenity_prelude as serenity;

use crate::bot::{Context, Error, util};
use crate::i18n;

/// Who am I? Who are you? Questions never asked.
#[poise::command(slash_command)]
pub async fn about(ctx: Context<'_>) -> Result<(), Error> {
    let locale = util::locale(&ctx);

    let embed = serenity::CreateEmbed::new()
        .title(i18n::t(&locale, "about-title"))
        .description(i18n::t_args(
            &locale,
            "about-description",
            &[("version", env!("CARGO_PKG_VERSION").into())],
        ))
        .thumbnail(ctx.cache().current_user().face())
        .colour(util::COLOR_MAIN);

    util::reply_embed(ctx, embed, false).await
}

#[cfg(test)]
mod tests {
    use crate::i18n;

    #[test]
    fn about_messages_exist_and_render_version() {
        let locale = i18n::resolve(None);
        assert_ne!(i18n::t(&locale, "about-title"), "about-title");
        let description = i18n::t_args(
            &locale,
            "about-description",
            &[("version", env!("CARGO_PKG_VERSION").into())],
        );
        assert!(
            description.contains(env!("CARGO_PKG_VERSION")),
            "got: {description}"
        );
        assert!(description.contains("github.com"), "got: {description}");
    }
}
