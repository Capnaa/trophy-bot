//! `/delete` — stub, implemented in batch C9 (docs/specs/implementation-plan.md).

use crate::bot::{Context, Error, util};

/// Delete a trophy from your server.
#[poise::command(slash_command, guild_only, default_member_permissions = "MANAGE_GUILD")]
pub async fn delete(ctx: Context<'_>) -> Result<(), Error> {
    util::reply_under_construction(ctx).await
}
