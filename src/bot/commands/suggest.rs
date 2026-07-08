//! `/suggest` — stub, implemented in batch C17 (docs/specs/implementation-plan.md).

use crate::bot::{Context, Error, util};

/// Suggest a feature or change for the bot
#[poise::command(slash_command, user_cooldown = 10)]
pub async fn suggest(ctx: Context<'_>) -> Result<(), Error> {
    util::reply_under_construction(ctx).await
}
