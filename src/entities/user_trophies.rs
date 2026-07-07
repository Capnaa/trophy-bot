//! `user_trophies` table — one row per individual award (schema.md).
//! Duplicates of `(user_id, trophy_id)` are required functionality (ADR 0002).

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "user_trophies")]
pub struct Model {
    /// UUIDv7 — time-ordered, gives "most recent first" ordering.
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    /// Denormalized on purpose for direct leaderboard queries.
    pub guild_id: i64,
    pub user_id: i64,
    pub trophy_id: Uuid,
    /// NULL for all imported legacy rows (never tracked); set for new awards.
    pub awarded_by: Option<i64>,
    /// Synthetic for imports.
    pub awarded_at: DateTime,
    pub created_at: DateTime,
    pub updated_at: DateTime,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
