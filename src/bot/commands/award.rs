//! `/award` — stub, implemented in batch C3 (docs/specs/implementation-plan.md).

use crate::bot::{Context, Error, util};

/// Award a trophy for an user.
#[poise::command(slash_command, guild_only, default_member_permissions = "MANAGE_GUILD")]
pub async fn award(ctx: Context<'_>) -> Result<(), Error> {
    util::reply_under_construction(ctx).await
}
