//! `role_rewards` table — automatic role assignment rules (schema.md).
//! `UNIQUE(guild_id, role_id)`; max-20-per-guild and duplicate-requirement
//! rules are app-level, not DB constraints.

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "role_rewards")]
pub struct Model {
    /// UUIDv7, app-generated.
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub guild_id: i64,
    /// Kept even if the Discord role is deleted (F24).
    pub role_id: i64,
    /// CHECK ≥ 1.
    pub requirement: i32,
    pub created_at: DateTime,
    pub updated_at: DateTime,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
