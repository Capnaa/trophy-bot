//! `/trophies` — stub, implemented in batch C4 (docs/specs/implementation-plan.md).

use crate::bot::{Context, Error, util};

/// See a list of trophies.
#[poise::command(slash_command, guild_only)]
pub async fn trophies(ctx: Context<'_>) -> Result<(), Error> {
    util::reply_under_construction(ctx).await
}
