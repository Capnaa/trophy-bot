//! `/leaderboard` — the server's score ranking.
//!
//! Spec: docs/specs/commands-user.md § /leaderboard. All rendering logic
//! lives in the shared `crate::bot::render` module (also used by the panel
//! updater), which implements fixes F13-F16.

use crate::bot::{panel_updater, render, util, Context, Error};

/// Shows the server's leaderboard.
#[poise::command(slash_command, guild_only)]
pub async fn leaderboard(
    ctx: Context<'_>,
    #[description = "Which page to show. Defaults to 1"] page: Option<i64>,
) -> Result<(), Error> {
    // Legacy parity: the reply is publicly deferred (member lookups can
    // exceed the 3-second interaction window on large boards).
    ctx.defer().await?;

    // Effective guild (guild_links): a linked guild's /leaderboard shows the
    // SOURCE guild's ranking it mirrors.
    let guild_id = util::effective_guild_id(&ctx).await?;

    let locale = util::locale(&ctx);
    let guild_name =
        panel_updater::guild_display_name(ctx.serenity_context(), guild_id, &locale).await;

    let embed = render::render_leaderboard(
        &ctx.data().db,
        ctx.serenity_context(),
        guild_id,
        &guild_name,
        page.unwrap_or(1),
        &locale,
        true,
    )
    .await?;

    util::reply_embed(ctx, embed, false).await
}
