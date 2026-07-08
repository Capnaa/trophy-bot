//! `/stats` — stub, implemented in batch C14 (docs/specs/implementation-plan.md).

use crate::bot::{Context, Error, util};

/// Look at the bot stats
#[poise::command(slash_command, user_cooldown = 10)]
pub async fn stats(ctx: Context<'_>) -> Result<(), Error> {
    util::reply_under_construction(ctx).await
}
