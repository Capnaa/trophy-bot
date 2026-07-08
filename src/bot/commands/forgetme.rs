//! `/forgetme` — stub, implemented in batch C16 (docs/specs/implementation-plan.md).

use crate::bot::{Context, Error, util};

/// Remove all images and data about your server from the bot and kick it.
#[poise::command(slash_command, guild_only, default_member_permissions = "ADMINISTRATOR")]
pub async fn forgetme(ctx: Context<'_>) -> Result<(), Error> {
    util::reply_under_construction(ctx).await
}
