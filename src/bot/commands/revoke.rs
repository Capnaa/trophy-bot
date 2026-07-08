//! `/revoke` — remove up to N copies of a trophy from a user (batch C6).
//!
//! Spec: docs/specs/commands-trophy-management.md §/revoke. Parity fixes:
//! - F1: the legacy `trophies.pop(id)` bug removed the LAST array element
//!   regardless of which trophy was requested, desyncing the array from the
//!   stored score. Here we delete exactly N `user_trophies` rows OF THE
//!   REQUESTED trophy, most recent first (`awarded_at` DESC, then UUIDv7 id
//!   DESC), in a single transaction. Score can never desync because it is
//!   always recomputed (`SUM(value)`, ADR 0006).
//! - F2: honest feedback — the reply states the REAL number of copies
//!   removed (which may be fewer than requested), and revoking from a user
//!   who holds none is an explicit informative error, not a fake
//!   "removed all" success (and it writes nothing, unlike the legacy no-op
//!   that created empty user records).
//! - Count validation mirrors /award (F8): 1–50 rejected, never coerced.
//! - §2 reward engine: reward roles are recomputed and applied (awaited)
//!   after the reply, exactly like /award.

use poise::serenity_prelude as serenity;
use sea_orm::{
    ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter, QueryOrder, QuerySelect,
    TransactionSession, TransactionTrait,
};
use uuid::Uuid;

use crate::bot::commands::award::{count_in_range, MAX_COUNT, MIN_COUNT};
use crate::bot::{reward_apply, resolver, util, Context, Error};
use crate::entities::user_trophies;
use crate::i18n;

/// F1: deletes up to `count` award rows of the REQUESTED trophy for the
/// user, most recent first (`awarded_at` DESC, UUIDv7 `id` DESC as the
/// tiebreaker — v7 ids are time-ordered). Runs in one transaction and
/// returns the number of rows actually removed, which is what the reply
/// reports (F2). Removing fewer than requested (or zero) is not an error.
pub(crate) async fn revoke_awards<C: TransactionTrait + ConnectionTrait>(
    db: &C,
    guild_id: i64,
    user_id: i64,
    trophy_id: Uuid,
    count: u64,
) -> anyhow::Result<u64> {
    let txn = db.begin().await?;

    let victim_ids: Vec<Uuid> = user_trophies::Entity::find()
        .select_only()
        .column(user_trophies::Column::Id)
        .filter(user_trophies::Column::GuildId.eq(guild_id))
        .filter(user_trophies::Column::UserId.eq(user_id))
        .filter(user_trophies::Column::TrophyId.eq(trophy_id))
        .order_by_desc(user_trophies::Column::AwardedAt)
        .order_by_desc(user_trophies::Column::Id)
        .limit(count)
        .into_tuple()
        .all(&txn)
        .await?;

    if victim_ids.is_empty() {
        // Nothing to remove — and unlike the legacy bot, nothing is written.
        txn.commit().await?;
        return Ok(0);
    }

    let deleted = user_trophies::Entity::delete_many()
        .filter(user_trophies::Column::Id.is_in(victim_ids))
        .exec(&txn)
        .await?
        .rows_affected;

    txn.commit().await?;
    Ok(deleted)
}

/// Revoke a trophy from an user.
#[poise::command(slash_command, guild_only, default_member_permissions = "MANAGE_GUILD")]
pub async fn revoke(
    ctx: Context<'_>,
    #[description = "Name of the trophy to revoke"]
    #[autocomplete = "resolver::autocomplete_trophy"]
    trophy: String,
    #[description = "User to revoke the trophy from"] user: serenity::User,
    #[description = "Number of trophies to revoke, defaults to 1"]
    #[min = 1]
    #[max = 50]
    count: Option<i64>,
) -> Result<(), Error> {
    let locale = util::locale(&ctx);
    let guild_id = ctx
        .guild_id()
        .ok_or_else(|| anyhow::anyhow!("guild_only command invoked outside a guild"))?;
    let db = &ctx.data().db;

    // Same policy as /award (F8): reject out-of-range counts server-side too.
    let count = count.unwrap_or(1);
    if !count_in_range(count) {
        return util::reply_error(
            ctx,
            i18n::t_args(
                &locale,
                "revoke-error-count",
                &[("min", MIN_COUNT.into()), ("max", MAX_COUNT.into())],
            ),
            true,
        )
        .await;
    }

    let Some(model) = resolver::resolve_trophy(db, guild_id.get() as i64, &trophy).await? else {
        return util::reply_error(
            ctx,
            i18n::t_args(&locale, "revoke-error-not-found", &[("input", trophy.into())]),
            true,
        )
        .await;
    };

    let removed = revoke_awards(
        db,
        guild_id.get() as i64,
        user.id.get() as i64,
        model.id,
        count as u64,
    )
    .await?;

    // F2: no copies held → explicit informative message, not a fake success.
    if removed == 0 {
        return util::reply_error(
            ctx,
            i18n::t_args(
                &locale,
                "revoke-error-none",
                &[
                    ("user", format!("<@{}>", user.id.get()).into()),
                    ("emoji", model.emoji.clone().into()),
                    ("name", model.name.clone().into()),
                ],
            ),
            true,
        )
        .await;
    }

    // F29: the score board changed — request a debounced panel refresh.
    ctx.data().panel_signal.notify(guild_id.get() as i64);

    // Reply first (the deletion is committed), then apply reward roles: the
    // Discord-side work can be slow and must never push the interaction past
    // its acknowledgement deadline.
    let description = i18n::t_args(
        &locale,
        "revoke-revoked",
        &[
            ("count", (removed as i64).into()),
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
    // committed revocation must not turn the already-sent success into an
    // error.
    if let Err(err) = reward_apply::apply_rewards(&ctx, guild_id, user.id).await {
        log::error!(
            "reward application failed after /revoke (guild={}, user={}): {err:#}",
            guild_id.get(),
            user.id.get()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sea_orm::{ActiveModelTrait, DatabaseConnection, PaginatorTrait, Set};

    use crate::domain::normalize::normalize_name;
    use crate::domain::queries;
    use crate::domain::test_support::{fresh_db, insert_guild, now};
    use crate::entities::trophies;

    async fn insert_trophy(db: &DatabaseConnection, guild_id: i64, name: &str, value: i32) -> Uuid {
        let id = Uuid::now_v7();
        trophies::ActiveModel {
            id: Set(id),
            guild_id: Set(guild_id),
            legacy_id: Set(None),
            creator_user_id: Set(None),
            name: Set(name.to_string()),
            normalized_name: Set(normalize_name(name)),
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

    /// Inserts one award row with an explicit `awarded_at`, returning its id.
    async fn insert_award(
        db: &DatabaseConnection,
        guild_id: i64,
        user_id: i64,
        trophy_id: Uuid,
        awarded_at: chrono::NaiveDateTime,
    ) -> Uuid {
        let id = Uuid::now_v7();
        user_trophies::ActiveModel {
            id: Set(id),
            guild_id: Set(guild_id),
            user_id: Set(user_id),
            trophy_id: Set(trophy_id),
            awarded_by: Set(Some(42)),
            awarded_at: Set(awarded_at),
            created_at: Set(awarded_at),
            updated_at: Set(awarded_at),
        }
        .insert(db)
        .await
        .expect("insert award");
        id
    }

    async fn remaining_ids(db: &DatabaseConnection, user_id: i64) -> Vec<Uuid> {
        user_trophies::Entity::find()
            .select_only()
            .column(user_trophies::Column::Id)
            .filter(user_trophies::Column::UserId.eq(user_id))
            .into_tuple()
            .all(db)
            .await
            .unwrap()
    }

    fn at(secs: i64) -> chrono::NaiveDateTime {
        chrono::DateTime::from_timestamp(1_700_000_000 + secs, 0)
            .unwrap()
            .naive_utc()
    }

    // --- F1: only the requested trophy is touched ---

    #[tokio::test]
    async fn revokes_only_the_requested_trophy() {
        let db = fresh_db().await;
        insert_guild(&db, 1).await;
        let gold = insert_trophy(&db, 1, "Gold", 10).await;
        let silver = insert_trophy(&db, 1, "Silver", 5).await;

        // Legacy pop-bug scenario: the LAST awarded trophy is Silver, but we
        // revoke Gold — Silver must survive.
        insert_award(&db, 1, 7, gold, at(0)).await;
        let silver_row = insert_award(&db, 1, 7, silver, at(10)).await;

        let removed = revoke_awards(&db, 1, 7, gold, 1).await.unwrap();

        assert_eq!(removed, 1);
        assert_eq!(remaining_ids(&db, 7).await, vec![silver_row], "Silver kept");
        // Score self-heals via SUM: only Silver's value remains.
        assert_eq!(queries::user_score(&db, 1, 7).await.unwrap(), 5);
    }

    #[tokio::test]
    async fn revokes_most_recent_copies_first() {
        let db = fresh_db().await;
        insert_guild(&db, 1).await;
        let gold = insert_trophy(&db, 1, "Gold", 10).await;

        let oldest = insert_award(&db, 1, 7, gold, at(0)).await;
        insert_award(&db, 1, 7, gold, at(10)).await;
        insert_award(&db, 1, 7, gold, at(20)).await;

        let removed = revoke_awards(&db, 1, 7, gold, 2).await.unwrap();

        assert_eq!(removed, 2);
        assert_eq!(
            remaining_ids(&db, 7).await,
            vec![oldest],
            "the two most recently awarded copies go first"
        );
    }

    #[tokio::test]
    async fn same_awarded_at_falls_back_to_uuid_order() {
        let db = fresh_db().await;
        insert_guild(&db, 1).await;
        let gold = insert_trophy(&db, 1, "Gold", 10).await;

        // Identical awarded_at (e.g. a bulk /award): UUIDv7 ids break the tie,
        // newest id removed first.
        let first = insert_award(&db, 1, 7, gold, at(0)).await;
        let second = insert_award(&db, 1, 7, gold, at(0)).await;
        assert!(second > first, "UUIDv7 ids are time-ordered");

        let removed = revoke_awards(&db, 1, 7, gold, 1).await.unwrap();

        assert_eq!(removed, 1);
        assert_eq!(remaining_ids(&db, 7).await, vec![first]);
    }

    // --- F2: honest counts ---

    #[tokio::test]
    async fn over_revoking_is_capped_and_reports_the_real_count() {
        let db = fresh_db().await;
        insert_guild(&db, 1).await;
        let gold = insert_trophy(&db, 1, "Gold", 10).await;
        insert_award(&db, 1, 7, gold, at(0)).await;
        insert_award(&db, 1, 7, gold, at(1)).await;

        let removed = revoke_awards(&db, 1, 7, gold, 50).await.unwrap();

        assert_eq!(removed, 2, "only what the user actually held");
        assert_eq!(queries::user_score(&db, 1, 7).await.unwrap(), 0);
    }

    #[tokio::test]
    async fn revoking_from_a_user_with_no_copies_removes_nothing() {
        let db = fresh_db().await;
        insert_guild(&db, 1).await;
        let gold = insert_trophy(&db, 1, "Gold", 10).await;
        let silver = insert_trophy(&db, 1, "Silver", 5).await;
        insert_award(&db, 1, 7, silver, at(0)).await;

        let removed = revoke_awards(&db, 1, 7, gold, 3).await.unwrap();

        assert_eq!(removed, 0, "F2: zero, not a fake 'all'");
        let total = user_trophies::Entity::find().count(&db).await.unwrap();
        assert_eq!(total, 1, "no rows created or removed by the no-op");
    }

    #[tokio::test]
    async fn other_users_and_guilds_are_untouched() {
        let db = fresh_db().await;
        insert_guild(&db, 1).await;
        insert_guild(&db, 2).await;
        let gold = insert_trophy(&db, 1, "Gold", 10).await;
        let gold2 = insert_trophy(&db, 2, "Gold", 10).await;

        insert_award(&db, 1, 7, gold, at(0)).await; // target
        let other_user = insert_award(&db, 1, 8, gold, at(1)).await;
        let other_guild = insert_award(&db, 2, 7, gold2, at(2)).await;

        let removed = revoke_awards(&db, 1, 7, gold, 50).await.unwrap();

        assert_eq!(removed, 1);
        let mut left = user_trophies::Entity::find()
            .select_only()
            .column(user_trophies::Column::Id)
            .into_tuple::<Uuid>()
            .all(&db)
            .await
            .unwrap();
        left.sort();
        let mut expected = vec![other_user, other_guild];
        expected.sort();
        assert_eq!(left, expected);
    }

    // --- count validation shared with /award (F8) ---

    #[test]
    fn count_bounds_match_award() {
        assert!(count_in_range(1) && count_in_range(50));
        assert!(!count_in_range(0) && !count_in_range(51));
    }

    // --- i18n catalog ---

    #[test]
    fn catalog_pluralizes_the_success_message() {
        let locale = i18n::resolve(None);
        let args = |count: i64| {
            i18n::t_args(
                &locale,
                "revoke-revoked",
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
        let many = args(2);
        assert!(many.contains("trophies"), "plural form: {many}");
        assert!(many.contains("Gold") && many.contains("<@7>"));
    }

    #[test]
    fn catalog_has_error_messages() {
        let locale = i18n::resolve(None);

        let count_error = i18n::t_args(
            &locale,
            "revoke-error-count",
            &[("min", MIN_COUNT.into()), ("max", MAX_COUNT.into())],
        );
        assert!(count_error.contains('1') && count_error.contains("50"), "{count_error}");

        let not_found =
            i18n::t_args(&locale, "revoke-error-not-found", &[("input", "Nope".into())]);
        assert!(not_found.contains("Nope"), "{not_found}");

        let none = i18n::t_args(
            &locale,
            "revoke-error-none",
            &[
                ("user", "<@7>".into()),
                ("emoji", "🏆".into()),
                ("name", "Gold".into()),
            ],
        );
        assert!(none.contains("<@7>") && none.contains("Gold"), "{none}");
    }
}
