//! `/export` — stub, implemented in batch C15 (docs/specs/implementation-plan.md).

use crate::bot::{Context, Error, util};

/// Export the bot's data
#[poise::command(slash_command, guild_only, default_member_permissions = "ADMINISTRATOR")]
pub async fn export(ctx: Context<'_>) -> Result<(), Error> {
    util::reply_under_construction(ctx).await
}
