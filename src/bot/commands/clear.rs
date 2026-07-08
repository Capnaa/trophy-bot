//! `/clear` — remove all trophies from a user and reset their score (batch C7).
//!
//! Spec: docs/specs/commands-trophy-management.md §/clear. Parity fixes:
//! - Rust target: `DELETE FROM user_trophies WHERE guild_id=? AND user_id=?`
//!   — no user row is created or needed (ADR 0006: score is always computed,
//!   so deleting the award rows IS the reset). The legacy bot instead wrote
//!   `trophies = []` / `trophyValue = 0`, creating empty records for unknown
//!   users as a side effect.
//! - §2 reward engine: reward roles are recomputed and APPLIED (awaited) via
//!   `crate::bot::reward_apply` after the delete — the legacy `doRewardRoles`
//!   was dead under discord.js v14, so roles were never stripped despite the
//!   score reset.
//! - QUIRK fix: the legacy option description was a copy-paste ("User to
//!   award the trophy to"); it now says what the command does.
//! - The success reply reports HOW MANY awards were cleared (the legacy reply
//!   reported nothing, and "clearing" a user with no data looked identical).
//!
//! Business logic lives in a plain testable function; the handler stays thin.

use poise::serenity_prelude as serenity;
use sea_orm::{ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter};

use crate::bot::{reward_apply, util, Context, Error};
use crate::entities::user_trophies;
use crate::i18n;

/// Deletes every award of `user_id` in `guild_id` and returns how many rows
/// were removed. A single DELETE statement — atomic on its own, and score
/// needs no fixup because it is always `SUM(value)` via JOIN (ADR 0006).
/// Clearing a user with no awards is a no-op returning 0.
pub(crate) async fn clear_awards(
    db: &impl ConnectionTrait,
    guild_id: i64,
    user_id: i64,
) -> anyhow::Result<u64> {
    let result = user_trophies::Entity::delete_many()
        .filter(user_trophies::Column::GuildId.eq(guild_id))
        .filter(user_trophies::Column::UserId.eq(user_id))
        .exec(db)
        .await?;
    Ok(result.rows_affected)
}

/// Clear all trophies and resets the score of an user to 0.
#[poise::command(slash_command, guild_only, default_member_permissions = "MANAGE_GUILD")]
pub async fn clear(
    ctx: Context<'_>,
    #[description = "User to clear all trophies from"] user: serenity::User,
) -> Result<(), Error> {
    let locale = util::locale(&ctx);
    let guild_id = ctx
        .guild_id()
        .ok_or_else(|| anyhow::anyhow!("guild_only command invoked outside a guild"))?;
    let db = &ctx.data().db;

    let cleared = clear_awards(db, guild_id.get() as i64, user.id.get() as i64).await?;

    // F29: the score board changed — request a debounced panel refresh.
    if cleared > 0 {
        ctx.data().panel_signal.notify(guild_id.get() as i64);
    }

    // Reply first (the delete is committed), then apply reward roles: the
    // Discord-side work (member fetch + role calls) can be slow and must
    // never push the interaction past its acknowledgement deadline.
    let description = i18n::t_args(
        &locale,
        "clear-cleared",
        &[
            ("count", (cleared as i64).into()),
            ("user", format!("<@{}>", user.id.get()).into()),
        ],
    );
    let embed = serenity::CreateEmbed::new()
        .colour(util::COLOR_SUCCESS)
        .description(description);
    util::reply_embed(ctx, embed, false).await?;

    // §2: awaited, idempotent, errors logged — an engine failure after the
    // committed clear must not turn the already-sent success into an error.
    // Score is now 0, so the engine strips every reward role.
    if let Err(err) = reward_apply::apply_rewards(&ctx, guild_id, user.id).await {
        log::error!(
            "reward application failed after /clear (guild={}, user={}): {err:#}",
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
    use uuid::Uuid;

    use crate::domain::normalize::normalize_name;
    use crate::domain::queries;
    use crate::domain::test_support::{fresh_db, insert_guild, now};
    use crate::entities::trophies;

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

    async fn award(db: &DatabaseConnection, guild_id: i64, user_id: i64, trophy_id: Uuid) {
        user_trophies::ActiveModel {
            id: Set(Uuid::now_v7()),
            guild_id: Set(guild_id),
            user_id: Set(user_id),
            trophy_id: Set(trophy_id),
            awarded_by: Set(None),
            awarded_at: Set(now()),
            created_at: Set(now()),
            updated_at: Set(now()),
        }
        .insert(db)
        .await
        .expect("insert award");
    }

    // --- clear_awards ---

    #[tokio::test]
    async fn deletes_every_award_of_the_user_and_reports_the_count() {
        let db = fresh_db().await;
        insert_guild(&db, 1).await;
        let gold = insert_trophy(&db, 1, 10).await;
        let shame = insert_trophy(&db, 1, -3).await;

        // Duplicates included (ADR 0002) — all of them must go.
        award(&db, 1, 7, gold).await;
        award(&db, 1, 7, gold).await;
        award(&db, 1, 7, shame).await;

        let cleared = clear_awards(&db, 1, 7).await.unwrap();
        assert_eq!(cleared, 3, "every row of the user is removed and counted");

        let remaining = user_trophies::Entity::find().count(&db).await.unwrap();
        assert_eq!(remaining, 0);
    }

    #[tokio::test]
    async fn score_recomputes_to_zero_after_clear() {
        let db = fresh_db().await;
        insert_guild(&db, 1).await;
        let trophy = insert_trophy(&db, 1, 25).await;
        award(&db, 1, 7, trophy).await;
        award(&db, 1, 7, trophy).await;
        assert_eq!(queries::user_score(&db, 1, 7).await.unwrap(), 50);

        clear_awards(&db, 1, 7).await.unwrap();

        // ADR 0006: nothing stored — the recomputed SUM is the reset.
        assert_eq!(queries::user_score(&db, 1, 7).await.unwrap(), 0);
    }

    #[tokio::test]
    async fn is_scoped_to_guild_and_user() {
        let db = fresh_db().await;
        insert_guild(&db, 1).await;
        insert_guild(&db, 2).await;
        let here = insert_trophy(&db, 1, 10).await;
        let elsewhere = insert_trophy(&db, 2, 10).await;

        award(&db, 1, 7, here).await; // target: cleared
        award(&db, 1, 8, here).await; // other user, same guild: kept
        award(&db, 2, 7, elsewhere).await; // same user, other guild: kept

        let cleared = clear_awards(&db, 1, 7).await.unwrap();
        assert_eq!(cleared, 1);

        assert_eq!(queries::user_score(&db, 1, 8).await.unwrap(), 10);
        assert_eq!(queries::user_score(&db, 2, 7).await.unwrap(), 10);
        assert_eq!(queries::user_score(&db, 1, 7).await.unwrap(), 0);
    }

    #[tokio::test]
    async fn clearing_a_user_with_no_awards_is_a_noop_returning_zero() {
        let db = fresh_db().await;
        insert_guild(&db, 1).await;

        let cleared = clear_awards(&db, 1, 7).await.unwrap();
        assert_eq!(cleared, 0, "unknown user: nothing to delete, no error");

        // Unlike the legacy bot, no empty user record is created — there is
        // no user table at all; absence of rows IS the zero state.
        let rows = user_trophies::Entity::find().count(&db).await.unwrap();
        assert_eq!(rows, 0);
    }

    // --- i18n catalog ---

    #[test]
    fn catalog_reports_the_cleared_count_with_plurals() {
        let locale = i18n::resolve(None);
        let args = |count: i64| {
            i18n::t_args(
                &locale,
                "clear-cleared",
                &[("count", count.into()), ("user", "<@7>".into())],
            )
        };

        let many = args(3);
        assert!(many.contains('3') && many.contains("<@7>"), "{many}");
        assert!(many.contains("trophies"), "plural form: {many}");

        let one = args(1);
        assert!(one.contains("trophy "), "singular form: {one}");

        let none = args(0);
        assert!(none.contains("<@7>"), "{none}");
        assert!(
            none.to_lowercase().contains("no trophies"),
            "zero variant tells the caller nothing was cleared: {none}"
        );
    }
}
