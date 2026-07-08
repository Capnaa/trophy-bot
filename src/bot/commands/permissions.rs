//! `/permissions` — stub, implemented in batch C17 (docs/specs/implementation-plan.md).

use crate::bot::{Context, Error, util};

/// Modify the permissions of a role. (Deprecated)
#[poise::command(slash_command, guild_only, default_member_permissions = "MANAGE_GUILD")]
pub async fn permissions(ctx: Context<'_>) -> Result<(), Error> {
    util::reply_under_construction(ctx).await
}
