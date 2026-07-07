//! `guilds` table — Discord server registrations (schema.md).

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "guilds")]
pub struct Model {
    /// Discord snowflake.
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: i64,
    /// Legacy `imsafe`; absent in legacy → false.
    pub is_safe: bool,
    pub created_at: DateTime,
    pub updated_at: DateTime,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
