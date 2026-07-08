//! `/revoke` — stub, implemented in batch C6 (docs/specs/implementation-plan.md).

use crate::bot::{Context, Error, util};

/// Revoke a trophy from an user.
#[poise::command(slash_command, guild_only, default_member_permissions = "MANAGE_GUILD")]
pub async fn revoke(ctx: Context<'_>) -> Result<(), Error> {
    util::reply_under_construction(ctx).await
}
