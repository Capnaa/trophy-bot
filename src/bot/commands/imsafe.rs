//! `/imsafe` — stub, implemented in batch C17 (docs/specs/implementation-plan.md).

use crate::bot::{Context, Error, util};

/// Confirms you're using Discord permissions instead of the deprecated custom permissions
#[poise::command(slash_command, guild_only, default_member_permissions = "MANAGE_GUILD")]
pub async fn imsafe(ctx: Context<'_>) -> Result<(), Error> {
    util::reply_under_construction(ctx).await
}
