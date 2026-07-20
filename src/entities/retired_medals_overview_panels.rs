//! `retired_medals_overview_panels` table — persistent per-guild "all
//! categories" panel listing every INACTIVE (retired) trophy (schema.md).
//! One panel per guild, enforced by the primary key — the inactive
//! counterpart of `medals_overview_panels`.

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "retired_medals_overview_panels")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub guild_id: i64,
    pub channel_id: i64,
    pub message_id: i64,
    /// Cross-guild link: NULL = render `guild_id`'s own catalog (default);
    /// set = render this OTHER guild's instead.
    pub source_guild_id: Option<i64>,
    pub created_at: DateTime,
    /// Doubles as "last successful render" (same convention as leaderboard_panels).
    pub updated_at: DateTime,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
