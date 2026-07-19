//! `guild_links` table — cross-guild panel-mirroring consent (schema.md).
//! A linked guild (`linked_guild_id`) can point at only one source guild at
//! a time (`UNIQUE(linked_guild_id)`); a source guild can be linked into
//! many guilds' panels. `accepted_at` doubles as the status flag: NULL =
//! pending, set = accepted.

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "guild_links")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub source_guild_id: i64,
    pub linked_guild_id: i64,
    pub requested_by: i64,
    pub accepted_by: Option<i64>,
    /// NULL = pending, set = accepted.
    pub accepted_at: Option<DateTime>,
    pub created_at: DateTime,
    pub updated_at: DateTime,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
