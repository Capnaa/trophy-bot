//! `/panel` — stub, implemented in batch C13 (docs/specs/implementation-plan.md).

use crate::bot::{Context, Error, util};

/// Create a leaderboard panel. You can only have one panel at a time.
#[poise::command(slash_command, guild_only, default_member_permissions = "MANAGE_GUILD")]
pub async fn panel(ctx: Context<'_>) -> Result<(), Error> {
    util::reply_under_construction(ctx).await
}
