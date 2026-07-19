//! Score and leaderboard queries (ADR 0006: no stored score — always computed
//! as `COALESCE(SUM(trophies.value), 0)` joined through `user_trophies`).
//!
//! Built with `sea_query` so the SQL renders correctly on both SQLite and
//! PostgreSQL (ADR 0003): the empty `Relation` enums on the entities mean the
//! join is expressed manually here rather than via `find_also_related`.

use sea_orm::sea_query::{Alias, Expr, ExprTrait, Order, Query, SelectStatement};
use sea_orm::{ConnectionTrait, DbErr};

use crate::entities::{trophies, user_trophies};

/// `SELECT ... FROM user_trophies INNER JOIN trophies ON trophies.id =
/// user_trophies.trophy_id WHERE user_trophies.guild_id = ?` — the shared
/// core of both queries below.
fn awards_joined_with_trophies(guild_id: i64) -> SelectStatement {
    Query::select()
        .from(user_trophies::Entity)
        .inner_join(
            trophies::Entity,
            Expr::col((trophies::Entity, trophies::Column::Id))
                .equals((user_trophies::Entity, user_trophies::Column::TrophyId)),
        )
        .and_where(
            Expr::col((user_trophies::Entity, user_trophies::Column::GuildId)).eq(guild_id),
        )
        .to_owned()
}

/// Total score of one user in one guild: `COALESCE(SUM(t.value), 0)` over all
/// their awards. A user with no awards scores 0.
pub async fn user_score(
    db: &impl ConnectionTrait,
    guild_id: i64,
    user_id: i64,
) -> Result<i64, DbErr> {
    let stmt = awards_joined_with_trophies(guild_id)
        .expr(
            Expr::col((trophies::Entity, trophies::Column::Value))
                .sum()
                .if_null(0i64),
        )
        .and_where(
            Expr::col((user_trophies::Entity, user_trophies::Column::UserId)).eq(user_id),
        )
        .to_owned();

    let row = db
        .query_one(&stmt)
        .await?
        .ok_or_else(|| DbErr::Custom("aggregate query returned no row".to_string()))?;
    row.try_get_by_index::<i64>(0)
}

/// Guild leaderboard: `(user_id, total)` pairs ordered by total descending
/// (ties broken by ascending `user_id` for determinism). Every user with at
/// least one award appears — including users whose total is 0.
pub async fn leaderboard(
    db: &impl ConnectionTrait,
    guild_id: i64,
) -> Result<Vec<(i64, i64)>, DbErr> {
    let total = Alias::new("total");
    let user_id_col = (user_trophies::Entity, user_trophies::Column::UserId);
    let stmt = awards_joined_with_trophies(guild_id)
        .column(user_id_col)
        .expr_as(
            Expr::col((trophies::Entity, trophies::Column::Value)).sum(),
            total.clone(),
        )
        .group_by_col(user_id_col)
        .order_by(total, Order::Desc)
        .order_by(user_id_col, Order::Asc)
        .to_owned();

    db.query_all(&stmt)
        .await?
        .iter()
        .map(|row| {
            Ok((
                row.try_get_by_index::<i64>(0)?,
                row.try_get_by_index::<i64>(1)?,
            ))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use sea_orm::{ActiveModelTrait, DatabaseConnection, Set};
    use uuid::Uuid;

    use crate::domain::test_support::{fresh_db, insert_guild, now};
    use crate::entities::{trophies, user_trophies};

    async fn insert_trophy(db: &DatabaseConnection, guild_id: i64, name: &str, value: i32) -> Uuid {
        let id = Uuid::now_v7();
        trophies::ActiveModel {
            id: Set(id),
            guild_id: Set(guild_id),
            legacy_id: Set(None),
            creator_user_id: Set(None),
            name: Set(name.to_string()),
            normalized_name: Set(crate::domain::normalize::normalize_name(name)),
            description: Set("No description provided".to_string()),
            emoji: Set("🏆".to_string()),
            value: Set(value),
            image: Set(None),
            dedication_user_id: Set(None),
            dedication_text: Set(None),
            details: Set("No details provided.".to_string()),
            signed: Set(false),
            category: Set(None),
            active: Set(true),
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

    #[tokio::test]
    async fn user_score_is_zero_without_awards() {
        let db = fresh_db().await;
        insert_guild(&db, 1).await;

        let score = user_score(&db, 1, 42).await.expect("query score");
        assert_eq!(score, 0, "user with no awards must score 0 (COALESCE)");
    }

    #[tokio::test]
    async fn user_score_sums_values_including_duplicates_and_negatives() {
        let db = fresh_db().await;
        insert_guild(&db, 1).await;
        let gold = insert_trophy(&db, 1, "Gold", 10).await;
        let shame = insert_trophy(&db, 1, "Shame", -3).await;

        // Duplicate awards of the same trophy must each count (ADR 0002).
        award(&db, 1, 42, gold).await;
        award(&db, 1, 42, gold).await;
        award(&db, 1, 42, shame).await;

        let score = user_score(&db, 1, 42).await.expect("query score");
        assert_eq!(score, 17, "10 + 10 - 3");
    }

    #[tokio::test]
    async fn user_score_is_scoped_to_guild_and_user() {
        let db = fresh_db().await;
        insert_guild(&db, 1).await;
        insert_guild(&db, 2).await;
        let t1 = insert_trophy(&db, 1, "Here", 10).await;
        let t2 = insert_trophy(&db, 2, "Elsewhere", 100).await;

        award(&db, 1, 42, t1).await;
        award(&db, 2, 42, t2).await; // same user, other guild
        award(&db, 1, 43, t1).await; // other user, same guild

        let score = user_score(&db, 1, 42).await.expect("query score");
        assert_eq!(score, 10, "only guild 1 awards for user 42 count");
    }

    #[tokio::test]
    async fn leaderboard_orders_by_total_descending() {
        let db = fresh_db().await;
        insert_guild(&db, 1).await;
        let small = insert_trophy(&db, 1, "Small", 5).await;
        let big = insert_trophy(&db, 1, "Big", 50).await;

        award(&db, 1, 10, small).await; // total 5
        award(&db, 1, 20, big).await; // total 50
        award(&db, 1, 30, small).await;
        award(&db, 1, 30, small).await; // total 10

        let board = leaderboard(&db, 1).await.expect("query leaderboard");
        assert_eq!(board, vec![(20, 50), (30, 10), (10, 5)]);
    }

    #[tokio::test]
    async fn leaderboard_includes_users_with_awards_but_zero_total() {
        let db = fresh_db().await;
        insert_guild(&db, 1).await;
        let zero = insert_trophy(&db, 1, "Participation", 0).await;
        let plus = insert_trophy(&db, 1, "Plus", 5).await;
        let minus = insert_trophy(&db, 1, "Minus", -5).await;

        award(&db, 1, 10, zero).await; // zero-value trophy → total 0
        award(&db, 1, 20, plus).await;
        award(&db, 1, 20, minus).await; // cancels out → total 0
        award(&db, 1, 30, plus).await; // total 5

        let board = leaderboard(&db, 1).await.expect("query leaderboard");
        assert_eq!(board.len(), 3, "zero-total users with awards must appear");
        assert_eq!(board[0], (30, 5));
        // Ties broken by ascending user_id for determinism.
        assert_eq!(&board[1..], &[(10, 0), (20, 0)]);
    }

    #[tokio::test]
    async fn leaderboard_is_scoped_to_guild_and_empty_when_no_awards() {
        let db = fresh_db().await;
        insert_guild(&db, 1).await;
        insert_guild(&db, 2).await;
        let other = insert_trophy(&db, 2, "Other", 7).await;
        award(&db, 2, 99, other).await;

        let board = leaderboard(&db, 1).await.expect("query leaderboard");
        assert!(board.is_empty(), "no awards in guild 1 → empty leaderboard");

        let board = leaderboard(&db, 2).await.expect("query leaderboard");
        assert_eq!(board, vec![(99, 7)]);
    }
}
