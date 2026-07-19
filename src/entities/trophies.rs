//! `trophies` table — trophy definitions (schema.md, ADR 0004/0005).

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "trophies")]
pub struct Model {
    /// UUIDv7, app-generated.
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub guild_id: i64,
    /// Old per-guild string ID ("1".."212"); NULL for post-cutover trophies.
    pub legacy_id: Option<String>,
    /// NULL for the legacy trophies without creator.
    pub creator_user_id: Option<i64>,
    pub name: String,
    /// App-maintained normalization key (ADR 0005), unique per guild.
    pub normalized_name: String,
    pub description: String,
    pub emoji: String,
    pub value: i32,
    /// Opaque filename in `images/`; NULL = no image.
    pub image: Option<String>,
    pub dedication_user_id: Option<i64>,
    pub dedication_text: Option<String>,
    pub details: String,
    pub signed: bool,
    /// Free-text grouping label; NULL = uncategorized (not on any panel).
    pub category: Option<String>,
    /// Inactive medals are excluded from `/award` but stay visible everywhere else.
    pub active: bool,
    pub created_at: DateTime,
    pub updated_at: DateTime,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
