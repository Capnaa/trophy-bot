//! `leaderboard_panels` table — persistent leaderboard messages (schema.md).
//! One panel per guild, enforced by the primary key (F30).

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "leaderboard_panels")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub guild_id: i64,
    pub channel_id: i64,
    pub message_id: i64,
    /// Cross-guild link: NULL = render `guild_id`'s own leaderboard
    /// (default); set = render this OTHER guild's leaderboard instead.
    pub source_guild_id: Option<i64>,
    pub created_at: DateTime,
    /// Doubles as "last successful render" for the F32 reconciliation sweep.
    pub updated_at: DateTime,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
