//! `/support` — static help-links embed (spec: docs/specs/commands-utility.md).
//!
//! Fix vs legacy: the reply is genuinely ephemeral (the legacy dispatcher's
//! public defer made `ephemeral: true` a no-op).

use poise::serenity_prelude as serenity;

use crate::bot::{Context, Error, util};
use crate::i18n;

/// You need extra help? Join our support server
#[poise::command(slash_command)]
pub async fn support(ctx: Context<'_>) -> Result<(), Error> {
    let locale = util::locale(&ctx);

    let embed = serenity::CreateEmbed::new()
        .title(i18n::t(&locale, "support-title"))
        .description(i18n::t(&locale, "support-description"))
        .thumbnail(ctx.cache().current_user().face())
        .colour(util::COLOR_MAIN);

    util::reply_embed(ctx, embed, true).await
}
