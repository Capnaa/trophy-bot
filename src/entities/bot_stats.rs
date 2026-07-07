//! `bot_stats` table — key/value counters (schema.md). Legacy counters are
//! imported once as historical record and never used for validation.

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "bot_stats")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    /// Counter name, e.g. `award`, `total`, `trophiesAwarded` (historical).
    #[sea_orm(unique)]
    pub name: String,
    pub total: i64,
    pub created_at: DateTime,
    pub updated_at: DateTime,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
