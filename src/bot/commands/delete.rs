//! `/delete` — delete a trophy from the guild (batch C9).
//!
//! Spec: docs/specs/commands-trophy-management.md §/delete. Parity fixes:
//! - F10: hard delete of the trophy row — the `user_trophies` FK
//!   `ON DELETE CASCADE` removes every award of it (no whole-users rewrite,
//!   no `cleanseTrophies` type-mismatch orphans); role rewards ARE recomputed
//!   for every affected user (the legacy bot never did); the image file is
//!   unlinked only when the trophy actually has one (no `./images/null`
//!   attempts) with failures logged, not swallowed.
//! - F12 (shared): trophy resolved by exact normalized name with autocomplete
//!   (`src/bot/resolver.rs`) — no numeric-ID branch, no path traversal.
//! - Spec Rust target: the destructive delete is CONFIRMED with a
//!   confirm/cancel button pair (built on the C16 button infrastructure in
//!   `src/bot/buttons.rs`, which also runs the actual deletion on confirm).
//!   The handler below only resolves the trophy and issues the warning; only
//!   the invoker may press the buttons, and they expire like /forgetme's.
//!   Legacy /delete was a one-shot un-confirmed hard delete — the added
//!   confirmation is a documented intentional delta (rust-parity-plan.md §4).
//!
//! Score needs no fixup (ADR 0006: never stored — every reader recomputes
//! `SUM(value)`, which drops automatically once the awards cascade away).
//! The global created-trophies counter of the legacy bot is gone too: counts
//! are derived by query (see /create, which increments nothing).
//!
//! Business logic lives in plain testable functions; the poise handler at
//! the bottom stays thin.

use poise::serenity_prelude as serenity;
use sea_orm::{
    ColumnTrait, ConnectionTrait, EntityTrait, PaginatorTrait, QueryFilter, QuerySelect,
    TransactionSession, TransactionTrait,
};
use uuid::Uuid;

use crate::bot::{buttons, resolver, util, Context, Error};
use crate::entities::{trophies, user_trophies};
use crate::i18n;

/// Distinct users holding at least one award of `trophy_id`. Must run BEFORE
/// the deletion: the FK cascade wipes the award rows, and these are exactly
/// the users whose score (and therefore reward roles) changes.
pub(crate) async fn affected_user_ids(
    db: &impl ConnectionTrait,
    trophy_id: Uuid,
) -> Result<Vec<i64>, sea_orm::DbErr> {
    user_trophies::Entity::find()
        .filter(user_trophies::Column::TrophyId.eq(trophy_id))
        .select_only()
        .column(user_trophies::Column::UserId)
        .distinct()
        .into_tuple()
        .all(db)
        .await
}

/// Hard-deletes the trophy and returns the distinct user ids that held it.
///
/// Both steps run in ONE transaction so the returned set is exactly the set
/// of users whose awards the cascade removed — an award landing between the
/// collect and the delete can neither be missed nor half-counted.
pub(crate) async fn delete_trophy<C: TransactionTrait>(
    db: &C,
    trophy_id: Uuid,
) -> anyhow::Result<Vec<i64>> {
    let txn = db.begin().await?;
    let affected = affected_user_ids(&txn, trophy_id).await?;
    trophies::Entity::delete_by_id(trophy_id).exec(&txn).await?;
    txn.commit().await?;
    Ok(affected)
}

/// Awards currently referencing `trophy_id` (what the FK cascade will wipe),
/// shown in the confirmation warning so the invoker knows the blast radius.
pub(crate) async fn award_count(
    db: &impl ConnectionTrait,
    trophy_id: Uuid,
) -> Result<u64, sea_orm::DbErr> {
    user_trophies::Entity::find()
        .filter(user_trophies::Column::TrophyId.eq(trophy_id))
        .count(db)
        .await
}

/// Delete a trophy from your server.
#[poise::command(slash_command, guild_only, default_member_permissions = "MANAGE_GUILD", required_permissions = "MANAGE_GUILD")]
pub async fn delete(
    ctx: Context<'_>,
    #[description = "Name of the trophy to delete"]
    #[autocomplete = "resolver::autocomplete_trophy"]
    trophy: String,
) -> Result<(), Error> {
    let locale = util::locale(&ctx);
    // Effective guild (guild_links): a linked guild's /delete targets the
    // SOURCE guild's trophy it mirrors, same as /create and /edit.
    let guild_id = util::effective_guild_id(&ctx).await?;
    let db = &ctx.data().db;

    let Some(model) = resolver::resolve_trophy_or_reply(
        ctx,
        guild_id.get() as i64,
        &trophy,
        "delete-error-not-found",
    )
    .await?
    else {
        return Ok(());
    };

    // Confirmation step (spec Rust target): nothing is deleted here. The
    // warning states how many awards the cascade will remove; the buttons
    // encode the invoker, the trophy UUID and the issue time, and are
    // consumed by `src/bot/buttons.rs::handle_trophy_delete`.
    let awards = award_count(db, model.id).await?;
    let issued_at = chrono::Utc::now().timestamp();

    let embed = serenity::CreateEmbed::new()
        .title(i18n::t_args(
            &locale,
            "delete-confirm-title",
            &[("emoji", model.emoji.clone().into()), ("name", model.name.clone().into())],
        ))
        .description(i18n::t_args(
            &locale,
            "delete-confirm-description",
            &[
                ("awards", awards.into()),
                ("seconds", buttons::CONFIRM_TIMEOUT_SECS.into()),
            ],
        ))
        .colour(util::COLOR_ERROR);

    let invoker = ctx.author().id.get();
    let components = vec![serenity::CreateActionRow::Buttons(vec![
        serenity::CreateButton::new(buttons::delete_custom_id(
            buttons::DeleteButton::Confirm,
            issued_at,
            invoker,
            model.id,
        ))
        .style(serenity::ButtonStyle::Danger)
        .emoji('🗑')
        .label(i18n::t(&locale, "delete-button-confirm")),
        serenity::CreateButton::new(buttons::delete_custom_id(
            buttons::DeleteButton::Cancel,
            issued_at,
            invoker,
            model.id,
        ))
        .style(serenity::ButtonStyle::Secondary)
        .label(i18n::t(&locale, "delete-button-cancel")),
    ])];

    ctx.send(
        poise::CreateReply::default()
            .embed(embed)
            .components(components),
    )
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sea_orm::{ActiveModelTrait, DatabaseConnection, Set};

    use crate::domain::normalize::normalize_name;
    use crate::domain::queries;
    use crate::domain::test_support::{fresh_db, insert_guild, now};

    async fn insert_trophy(
        db: &DatabaseConnection,
        guild_id: i64,
        name: &str,
        value: i32,
        image: Option<&str>,
    ) -> trophies::Model {
        trophies::ActiveModel {
            id: Set(Uuid::now_v7()),
            guild_id: Set(guild_id),
            legacy_id: Set(None),
            creator_user_id: Set(None),
            name: Set(name.to_string()),
            normalized_name: Set(normalize_name(name)),
            description: Set("d".into()),
            emoji: Set("🏆".into()),
            value: Set(value),
            image: Set(image.map(str::to_string)),
            dedication_user_id: Set(None),
            dedication_text: Set(None),
            details: Set("d".into()),
            signed: Set(false),
            category: Set(None),
            active: Set(true),
            created_at: Set(now()),
            updated_at: Set(now()),
        }
        .insert(db)
        .await
        .expect("insert trophy")
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

    // --- affected_user_ids ---

    #[tokio::test]
    async fn affected_users_are_distinct_and_scoped_to_the_trophy() {
        let db = fresh_db().await;
        insert_guild(&db, 1).await;
        let doomed = insert_trophy(&db, 1, "Doomed", 10, None).await;
        let other = insert_trophy(&db, 1, "Other", 5, None).await;

        award(&db, 1, 42, doomed.id).await;
        award(&db, 1, 42, doomed.id).await; // duplicate award → one entry
        award(&db, 1, 43, doomed.id).await;
        award(&db, 1, 99, other.id).await; // different trophy → excluded

        let mut affected = affected_user_ids(&db, doomed.id).await.unwrap();
        affected.sort_unstable();
        assert_eq!(affected, vec![42, 43]);
    }

    #[tokio::test]
    async fn affected_users_empty_for_an_unawarded_trophy() {
        let db = fresh_db().await;
        insert_guild(&db, 1).await;
        let unused = insert_trophy(&db, 1, "Unused", 10, None).await;

        assert!(affected_user_ids(&db, unused.id).await.unwrap().is_empty());
    }

    // --- delete_trophy ---

    #[tokio::test]
    async fn delete_removes_the_trophy_and_cascades_its_awards_only() {
        let db = fresh_db().await;
        insert_guild(&db, 1).await;
        let doomed = insert_trophy(&db, 1, "Doomed", 10, None).await;
        let kept = insert_trophy(&db, 1, "Kept", 5, None).await;
        award(&db, 1, 42, doomed.id).await;
        award(&db, 1, 42, kept.id).await;

        let affected = delete_trophy(&db, doomed.id).await.unwrap();
        assert_eq!(affected, vec![42], "holder collected before the cascade");

        assert!(
            trophies::Entity::find_by_id(doomed.id).one(&db).await.unwrap().is_none(),
            "hard delete: the row is gone, not tombstoned"
        );
        assert!(trophies::Entity::find_by_id(kept.id).one(&db).await.unwrap().is_some());

        let remaining = user_trophies::Entity::find().all(&db).await.unwrap();
        assert_eq!(remaining.len(), 1, "only the doomed trophy's awards cascade away");
        assert_eq!(remaining[0].trophy_id, kept.id);
    }

    #[tokio::test]
    async fn scores_drop_automatically_after_delete() {
        let db = fresh_db().await;
        insert_guild(&db, 1).await;
        let doomed = insert_trophy(&db, 1, "Doomed", 10, None).await;
        let kept = insert_trophy(&db, 1, "Kept", 3, None).await;
        award(&db, 1, 42, doomed.id).await;
        award(&db, 1, 42, doomed.id).await;
        award(&db, 1, 42, kept.id).await; // 10 + 10 + 3 = 23

        delete_trophy(&db, doomed.id).await.unwrap();

        // ADR 0006: no stored score to fix up — the SUM just shrinks.
        assert_eq!(queries::user_score(&db, 1, 42).await.unwrap(), 3);
    }

    #[tokio::test]
    async fn deleting_a_trophy_nobody_holds_affects_no_users() {
        let db = fresh_db().await;
        insert_guild(&db, 1).await;
        let unused = insert_trophy(&db, 1, "Unused", 10, None).await;

        let affected = delete_trophy(&db, unused.id).await.unwrap();
        assert!(affected.is_empty());
        assert!(trophies::Entity::find_by_id(unused.id).one(&db).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn affected_users_from_delete_feed_the_reward_recompute() {
        // The set delete_trophy returns must be exactly what target_for_user
        // needs recomputing for: after the cascade the user's target shrinks.
        let db = fresh_db().await;
        insert_guild(&db, 1).await;
        crate::entities::role_rewards::ActiveModel {
            id: Set(Uuid::now_v7()),
            guild_id: Set(1),
            role_id: Set(500),
            requirement: Set(10),
            created_at: Set(now()),
            updated_at: Set(now()),
        }
        .insert(&db)
        .await
        .expect("insert reward");

        let doomed = insert_trophy(&db, 1, "Doomed", 10, None).await;
        award(&db, 1, 42, doomed.id).await; // score 10 → role 500 met

        let (before, _) = crate::bot::reward_apply::target_for_user(&db, 1, 42)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(before, vec![500]);

        let affected = delete_trophy(&db, doomed.id).await.unwrap();
        assert_eq!(affected, vec![42]);

        let (after, configured) = crate::bot::reward_apply::target_for_user(&db, 1, 42)
            .await
            .unwrap()
            .unwrap();
        assert!(after.is_empty(), "score fell to 0: reward no longer met");
        assert_eq!(configured, vec![500], "stale role stays removable");
    }

    // --- award_count (confirmation blast radius) ---

    #[tokio::test]
    async fn award_count_counts_every_copy_of_the_trophy_only() {
        let db = fresh_db().await;
        insert_guild(&db, 1).await;
        let doomed = insert_trophy(&db, 1, "Doomed", 10, None).await;
        let other = insert_trophy(&db, 1, "Other", 5, None).await;

        assert_eq!(award_count(&db, doomed.id).await.unwrap(), 0);

        award(&db, 1, 42, doomed.id).await;
        award(&db, 1, 42, doomed.id).await; // duplicates count individually
        award(&db, 1, 43, doomed.id).await;
        award(&db, 1, 99, other.id).await; // other trophy → excluded

        assert_eq!(award_count(&db, doomed.id).await.unwrap(), 3);
    }

    /// Regression guard for the confirmation step (spec Rust target): the
    /// slash-command handler must only WARN — the actual deletion runs from
    /// the button handler in `src/bot/buttons.rs` after an explicit confirm.
    #[test]
    fn slash_handler_never_deletes_directly() {
        let src = include_str!("delete.rs");
        let handler = src
            .split("pub async fn delete")
            .nth(1)
            .expect("delete handler exists");
        // Cut at the end of the handler function (next top-level item).
        let handler = &handler[..handler.find("\n#[cfg(test)]").unwrap_or(handler.len())];
        let forbidden = concat!("delete_", "trophy(");
        assert!(
            !handler.contains(forbidden),
            "/delete must not hard-delete without the button confirmation"
        );
        assert!(
            handler.contains("delete_custom_id"),
            "/delete must issue the confirmation buttons"
        );
    }

    // --- i18n catalog ---

    #[test]
    fn catalog_has_success_and_not_found_messages() {
        let locale = i18n::resolve(None);

        let success = i18n::t_args(
            &locale,
            "delete-success",
            &[("emoji", "🏅".into()), ("name", "Gold".into())],
        );
        assert!(success.contains("🏅") && success.contains("Gold"), "{success}");

        let not_found =
            i18n::t_args(&locale, "delete-error-not-found", &[("input", "Nope".into())]);
        assert!(not_found.contains("Nope"), "{not_found}");
    }

    #[test]
    fn catalog_has_confirmation_messages() {
        let locale = i18n::resolve(None);

        let title = i18n::t_args(
            &locale,
            "delete-confirm-title",
            &[("emoji", "🏅".into()), ("name", "Gold".into())],
        );
        assert!(title.contains("Gold"), "{title}");

        // The warning states the blast radius and the expiry window, with
        // singular/plural/zero award counts all rendering.
        for (awards, needle) in [(0i64, "nobody"), (1, "1 existing award"), (3, "3 existing awards")]
        {
            let description = i18n::t_args(
                &locale,
                "delete-confirm-description",
                &[("awards", awards.into()), ("seconds", 60.into())],
            );
            let plain: String = description.replace(['\u{2068}', '\u{2069}'], "");
            assert!(plain.contains(needle), "awards={awards}: {plain}");
            assert!(plain.contains("60"), "expiry shown: {plain}");
        }

        for key in ["delete-button-confirm", "delete-button-cancel"] {
            assert_ne!(i18n::t(&locale, key), key, "missing Fluent key {key}");
        }
    }
}
