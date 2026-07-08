//! `/show` — stub, implemented in batch C2 (docs/specs/implementation-plan.md).

use crate::bot::{Context, Error, util};

/// Show a trophy
#[poise::command(slash_command, guild_only)]
pub async fn show(ctx: Context<'_>) -> Result<(), Error> {
    util::reply_under_construction(ctx).await
}
