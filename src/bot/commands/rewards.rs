//! `/rewards` — stub, implemented in batch C12 (docs/specs/implementation-plan.md).

use crate::bot::{Context, Error, util};

/// Manage the role rewards of your server.
#[poise::command(slash_command, guild_only, default_member_permissions = "MANAGE_GUILD")]
pub async fn rewards(ctx: Context<'_>) -> Result<(), Error> {
    util::reply_under_construction(ctx).await
}
