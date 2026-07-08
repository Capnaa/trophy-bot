//! `/clear` — stub, implemented in batch C7 (docs/specs/implementation-plan.md).

use crate::bot::{Context, Error, util};

/// Clear all trophies and resets the score of an user to 0.
#[poise::command(slash_command, guild_only, default_member_permissions = "MANAGE_GUILD")]
pub async fn clear(ctx: Context<'_>) -> Result<(), Error> {
    util::reply_under_construction(ctx).await
}
