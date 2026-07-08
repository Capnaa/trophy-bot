//! `/support` — stub, implemented in batch C17 (docs/specs/implementation-plan.md).

use crate::bot::{Context, Error, util};

/// You need extra help? Join our support server
#[poise::command(slash_command)]
pub async fn support(ctx: Context<'_>) -> Result<(), Error> {
    util::reply_under_construction(ctx).await
}
