//! `/award` — award N copies of a trophy to a user (batch C3).
//!
//! Spec: docs/specs/commands-trophy-management.md §/award. Parity fixes:
//! - F8: `count` outside 1–50 is REJECTED with a clear localized error (the
//!   legacy bot silently coerced sub-1 values to 1 and printed a misleading
//!   "between 0 and 50" message for the high bound).
//! - F9: `awarded_by` is recorded on every new award row (NULL only exists
//!   on imported legacy rows).
//! - F12: trophy resolved by exact normalized name with autocomplete
//!   (`src/bot/resolver.rs`) — no numeric-ID branch, no path traversal, no
//!   substring surprises.
//! - §2 reward engine: role rewards are recomputed and APPLIED (awaited)
//!   via `crate::bot::reward_apply` after the awards commit — the legacy
//!   `doRewardRoles` was dead code under discord.js v14.
//!
//! Score is never stored (ADR 0006): awarding is just inserting N
//! `user_trophies` rows in one transaction; every reader recomputes
//! `SUM(value)`. Business logic lives in plain testable functions.

use poise::serenity_prelude as serenity;
use sea_orm::{ConnectionTrait, EntityTrait, Set, TransactionSession, TransactionTrait};
use uuid::Uuid;

use crate::bot::{reward_apply, resolver, util, Context, Error};
use crate::entities::user_trophies;
use crate::i18n;

/// Inclusive count bounds (legacy limit, kept — rust-parity-plan F8).
pub(crate) const MIN_COUNT: i64 = 1;
pub(crate) const MAX_COUNT: i64 = 50;

/// F8: the only accepted counts are 1..=50 — anything else is an error, not
/// a silent coercion.
pub(crate) fn count_in_range(count: i64) -> bool {
    (MIN_COUNT..=MAX_COUNT).contains(&count)
}

/// Inserts `count` award rows (one row per copy — duplicates are the
/// feature, ADR 0002) in a single transaction, recording who awarded (F9).
/// UUIDv7 keys keep the rows time-ordered.
pub(crate) async fn insert_awards<C: TransactionTrait + ConnectionTrait>(
    db: &C,
    guild_id: i64,
    user_id: i64,
    trophy_id: Uuid,
    count: u32,
    awarded_by: i64,
) -> anyhow::Result<()> {
    let now = chrono::Utc::now().naive_utc();
    let rows = (0..count).map(|_| user_trophies::ActiveModel {
        id: Set(Uuid::now_v7()),
        guild_id: Set(guild_id),
        user_id: Set(user_id),
        trophy_id: Set(trophy_id),
        awarded_by: Set(Some(awarded_by)),
        awarded_at: Set(now),
        created_at: Set(now),
        updated_at: Set(now),
    });

    let txn = db.begin().await?;
    user_trophies::Entity::insert_many(rows)
        .exec_without_returning(&txn)
        .await?;
    txn.commit().await?;
    Ok(())
}

/// Award a trophy for an user.
#[poise::command(slash_command, guild_only, default_member_permissions = "MANAGE_GUILD", required_permissions = "MANAGE_GUILD")]
pub async fn award(
    ctx: Context<'_>,
    #[description = "Name of the trophy to award"]
    #[autocomplete = "resolver::autocomplete_trophy"]
    trophy: String,
    #[description = "User to award the trophy to"] user: serenity::User,
    #[description = "Number of trophies to award, defaults to 1"]
    #[min = 1]
    #[max = 50]
    count: Option<i64>,
) -> Result<(), Error> {
    let locale = util::locale(&ctx);
    let guild_id = util::require_guild_id(&ctx)?;
    let db = &ctx.data().db;

    // F8: reject out-of-range counts server-side too (the Discord client
    // already enforces the min/max option bounds).
    let count = count.unwrap_or(1);
    if !count_in_range(count) {
        return util::reply_error(
            ctx,
            i18n::t_args(
                &locale,
                "award-error-count",
                &[("min", MIN_COUNT.into()), ("max", MAX_COUNT.into())],
            ),
            true,
        )
        .await;
    }

    let Some(model) = resolver::resolve_trophy_or_reply(
        ctx,
        guild_id.get() as i64,
        &trophy,
        "award-error-not-found",
    )
    .await?
    else {
        return Ok(());
    };

    insert_awards(
        db,
        guild_id.get() as i64,
        user.id.get() as i64,
        model.id,
        count as u32,
        ctx.author().id.get() as i64,
    )
    .await?;

    // F29: the score board changed — request a debounced panel refresh.
    ctx.data().panel_signal.notify(guild_id.get() as i64);

    // Reply first (the awards are committed), then apply reward roles: the
    // Discord-side work (member fetch + role calls) can be slow and must
    // never push the interaction past its acknowledgement deadline.
    let description = i18n::t_args(
        &locale,
        "award-awarded",
        &[
            ("count", count.into()),
            ("emoji", model.emoji.clone().into()),
            ("name", model.name.clone().into()),
            ("user", format!("<@{}>", user.id.get()).into()),
        ],
    );
    let embed = serenity::CreateEmbed::new()
        .colour(util::COLOR_SUCCESS)
        .description(description);
    util::reply_embed(ctx, embed, false).await?;

    // §2: awaited, idempotent, errors logged — an engine failure after the
    // committed award must not turn the already-sent success into an error.
    if let Err(err) = reward_apply::apply_rewards(&ctx, guild_id, user.id).await {
        log::error!(
            "reward application failed after /award (guild={}, user={}): {err:#}",
            guild_id.get(),
            user.id.get()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sea_orm::{ActiveModelTrait, ColumnTrait, DatabaseConnection, PaginatorTrait, QueryFilter};

    use crate::domain::normalize::normalize_name;
    use crate::domain::queries;
    use crate::domain::test_support::{fresh_db, insert_guild, now};
    use crate::entities::trophies;

    // --- count validation (F8) ---

    #[test]
    fn counts_one_through_fifty_are_accepted() {
        for count in [1, 2, 25, 50] {
            assert!(count_in_range(count), "count {count} must be accepted");
        }
    }

    #[test]
    fn out_of_range_counts_are_rejected_not_coerced() {
        for count in [i64::MIN, -1, 0, 51, i64::MAX] {
            assert!(!count_in_range(count), "count {count} must be rejected");
        }
    }

    // --- insert_awards ---

    async fn insert_trophy(db: &DatabaseConnection, guild_id: i64, value: i32) -> Uuid {
        let id = Uuid::now_v7();
        trophies::ActiveModel {
            id: Set(id),
            guild_id: Set(guild_id),
            legacy_id: Set(None),
            creator_user_id: Set(None),
            name: Set(format!("Trophy {id}")),
            normalized_name: Set(normalize_name(&format!("Trophy {id}"))),
            description: Set("d".into()),
            emoji: Set("🏆".into()),
            value: Set(value),
            image: Set(None),
            dedication_user_id: Set(None),
            dedication_text: Set(None),
            details: Set("d".into()),
            signed: Set(false),
            created_at: Set(now()),
            updated_at: Set(now()),
        }
        .insert(db)
        .await
        .expect("insert trophy");
        id
    }

    #[tokio::test]
    async fn inserts_one_row_per_copy_recording_awarded_by() {
        let db = fresh_db().await;
        insert_guild(&db, 1).await;
        let trophy = insert_trophy(&db, 1, 10).await;

        insert_awards(&db, 1, 7, trophy, 3, 42).await.unwrap();

        let rows = user_trophies::Entity::find()
            .filter(user_trophies::Column::GuildId.eq(1))
            .filter(user_trophies::Column::UserId.eq(7))
            .all(&db)
            .await
            .unwrap();
        assert_eq!(rows.len(), 3, "bulk award = N rows (duplicates allowed)");
        for row in &rows {
            assert_eq!(row.trophy_id, trophy);
            assert_eq!(row.awarded_by, Some(42), "F9: awarded_by recorded");
        }
    }

    #[tokio::test]
    async fn score_reflects_value_times_count() {
        let db = fresh_db().await;
        insert_guild(&db, 1).await;
        let trophy = insert_trophy(&db, 1, -25).await;

        insert_awards(&db, 1, 7, trophy, 4, 42).await.unwrap();

        // ADR 0006: nothing stored — the recomputed SUM is the score.
        assert_eq!(queries::user_score(&db, 1, 7).await.unwrap(), -100);
    }

    #[tokio::test]
    async fn repeated_awards_of_the_same_trophy_accumulate() {
        let db = fresh_db().await;
        insert_guild(&db, 1).await;
        let trophy = insert_trophy(&db, 1, 10).await;

        insert_awards(&db, 1, 7, trophy, 1, 42).await.unwrap();
        insert_awards(&db, 1, 7, trophy, 2, 43).await.unwrap();

        let total = user_trophies::Entity::find()
            .filter(user_trophies::Column::UserId.eq(7))
            .count(&db)
            .await
            .unwrap();
        assert_eq!(total, 3);
        assert_eq!(queries::user_score(&db, 1, 7).await.unwrap(), 30);
    }

    #[tokio::test]
    async fn maximum_bulk_award_of_fifty_works() {
        let db = fresh_db().await;
        insert_guild(&db, 1).await;
        let trophy = insert_trophy(&db, 1, 2).await;

        insert_awards(&db, 1, 7, trophy, 50, 42).await.unwrap();

        assert_eq!(queries::user_score(&db, 1, 7).await.unwrap(), 100);
    }

    // --- i18n catalog ---

    #[test]
    fn catalog_pluralizes_the_success_message() {
        let locale = i18n::resolve(None);
        let args = |count: i64| {
            i18n::t_args(
                &locale,
                "award-awarded",
                &[
                    ("count", count.into()),
                    ("emoji", "🏆".into()),
                    ("name", "Gold".into()),
                    ("user", "<@7>".into()),
                ],
            )
        };
        let one = args(1);
        assert!(one.contains("trophy "), "singular form: {one}");
        let many = args(5);
        assert!(many.contains("trophies"), "plural form: {many}");
        assert!(many.contains("Gold") && many.contains("<@7>"));
    }

    #[test]
    fn catalog_has_error_messages() {
        let locale = i18n::resolve(None);
        let count_error = i18n::t_args(
            &locale,
            "award-error-count",
            &[("min", MIN_COUNT.into()), ("max", MAX_COUNT.into())],
        );
        assert!(count_error.contains('1') && count_error.contains("50"), "{count_error}");

        let not_found =
            i18n::t_args(&locale, "award-error-not-found", &[("input", "Nope".into())]);
        assert!(not_found.contains("Nope"), "{not_found}");
    }
}
