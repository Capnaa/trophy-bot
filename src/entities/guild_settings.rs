//! `guild_settings` table — one row per guild, typed nullable columns
//! (schema.md). NULL = "not explicitly set" → code-side default, mirroring
//! legacy `getSetting` semantics.

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "guild_settings")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub guild_id: i64,
    /// NULL = default 2 (Mention Only in Server).
    pub dedication_display: Option<i16>,
    /// NULL = default 1 (Only Highest Reward).
    pub stack_roles: Option<i16>,
    /// NULL = default 1 (Show Unused Trophies).
    pub hide_unused_trophies: Option<i16>,
    /// NULL = default 0 (Hide Quit Users).
    pub hide_quit_users: Option<i16>,
    /// NULL = default 0 (Mention).
    pub leaderboard_format: Option<i16>,
    pub created_at: DateTime,
    pub updated_at: DateTime,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
