//! `/invite` — stub, implemented in batch C17 (docs/specs/implementation-plan.md).

use crate::bot::{Context, Error, util};

/// Invite the bot to your server!
#[poise::command(slash_command)]
pub async fn invite(ctx: Context<'_>) -> Result<(), Error> {
    util::reply_under_construction(ctx).await
}
