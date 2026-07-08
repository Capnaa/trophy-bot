//! `/leaderboard` — stub, implemented in batch C5 (docs/specs/implementation-plan.md).

use crate::bot::{Context, Error, util};

/// Shows the server's leaderboard.
#[poise::command(slash_command, guild_only)]
pub async fn leaderboard(ctx: Context<'_>) -> Result<(), Error> {
    util::reply_under_construction(ctx).await
}
