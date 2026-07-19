//! `active_medals_panels` table — persistent per-category medal catalog
//! messages (schema.md). One panel per category per guild, enforced by the
//! `(guild_id, category)` unique index.

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "active_medals_panels")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub guild_id: i64,
    pub category: String,
    pub channel_id: i64,
    pub message_id: i64,
    pub created_at: DateTime,
    /// Doubles as "last successful render" (same convention as leaderboard_panels).
    pub updated_at: DateTime,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
