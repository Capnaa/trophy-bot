//! `/edit` — stub, implemented in batch C8 (docs/specs/implementation-plan.md).

use crate::bot::{Context, Error, util};

/// Edit an existing trophy for your server.
#[poise::command(slash_command, guild_only, default_member_permissions = "MANAGE_GUILD")]
pub async fn edit(ctx: Context<'_>) -> Result<(), Error> {
    util::reply_under_construction(ctx).await
}
