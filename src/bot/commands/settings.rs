//! `/settings` — stub, implemented in batch C11 (docs/specs/implementation-plan.md).

use crate::bot::{Context, Error, util};

/// Modify the server settings for the bot.
#[poise::command(slash_command, guild_only, default_member_permissions = "MANAGE_GUILD")]
pub async fn settings(ctx: Context<'_>) -> Result<(), Error> {
    util::reply_under_construction(ctx).await
}
