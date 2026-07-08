//! `/help` — stub, implemented in batch C17 (docs/specs/implementation-plan.md).

use crate::bot::{Context, Error, util};

/// Stop it! Get some help!
#[poise::command(slash_command)]
pub async fn help(ctx: Context<'_>) -> Result<(), Error> {
    util::reply_under_construction(ctx).await
}
