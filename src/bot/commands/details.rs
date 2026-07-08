//! `/details` — stub, implemented in batch C10 (docs/specs/implementation-plan.md).

use crate::bot::{Context, Error, util};

/// Shows the details of a trophy.
#[poise::command(slash_command, guild_only, default_member_permissions = "MANAGE_GUILD")]
pub async fn details(ctx: Context<'_>) -> Result<(), Error> {
    util::reply_under_construction(ctx).await
}
