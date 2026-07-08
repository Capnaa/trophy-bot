//! Shared helpers for command implementations: embed colors, locale
//! resolution and reply shortcuts. Every command batch builds on these.

use poise::serenity_prelude as serenity;

use crate::bot::{Context, Error};
use crate::i18n::{self, LanguageIdentifier};

/// Main brand color (`#0096FF`, legacy `color.main`).
pub const COLOR_MAIN: u32 = 0x0096FF;
/// Error color (`#E02D44`, legacy `color.error`).
pub const COLOR_ERROR: u32 = 0xE02D44;
/// Success color (used by award/create confirmations in later batches).
#[allow(dead_code)]
pub const COLOR_SUCCESS: u32 = 0x2ECC71;

/// Resolves the Fluent locale for this interaction (exact tag → language
/// prefix → `en-US`). The ctx-locale shortcut every command uses.
pub fn locale(ctx: &Context<'_>) -> LanguageIdentifier {
    i18n::resolve(ctx.locale())
}

/// Sends (or follows up with) a single-embed reply. `ephemeral` must be
/// chosen explicitly by the caller — no hidden default.
pub async fn reply_embed(
    ctx: Context<'_>,
    embed: serenity::CreateEmbed,
    ephemeral: bool,
) -> Result<(), Error> {
    ctx.send(poise::CreateReply::default().embed(embed).ephemeral(ephemeral))
        .await?;
    Ok(())
}

/// Sends a localized-description error embed (error color + localized title).
/// `ephemeral` must be chosen explicitly; error replies are ephemeral unless
/// a spec says otherwise.
pub async fn reply_error(
    ctx: Context<'_>,
    description: impl Into<String>,
    ephemeral: bool,
) -> Result<(), Error> {
    let locale = locale(&ctx);
    let embed = serenity::CreateEmbed::new()
        .title(i18n::t(&locale, "common-error-title"))
        .description(description.into())
        .colour(COLOR_ERROR);
    reply_embed(ctx, embed, ephemeral).await
}

/// Standard stub reply for commands not yet implemented (always ephemeral).
pub async fn reply_under_construction(ctx: Context<'_>) -> Result<(), Error> {
    let locale = locale(&ctx);
    let embed = serenity::CreateEmbed::new()
        .title(i18n::t(&locale, "common-under-construction-title"))
        .description(i18n::t(&locale, "common-under-construction"))
        .colour(COLOR_MAIN);
    reply_embed(ctx, embed, true).await
}
