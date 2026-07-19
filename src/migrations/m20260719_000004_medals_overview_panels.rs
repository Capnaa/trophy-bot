//! Adds `medals_overview_panels`: one auto-updating panel per guild showing
//! every active, categorized trophy sectioned by category (all categories
//! combined into one message). Additive-only, applies cleanly on top of the
//! already-migrated schema.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table("medals_overview_panels")
                    .col(
                        ColumnDef::new("guild_id")
                            .big_integer()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new("channel_id").big_integer().not_null())
                    .col(ColumnDef::new("message_id").big_integer().not_null())
                    .col(ColumnDef::new("source_guild_id").big_integer().null())
                    .col(ColumnDef::new("created_at").timestamp().not_null())
                    .col(ColumnDef::new("updated_at").timestamp().not_null())
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_medals_overview_panels_guild")
                            .from("medals_overview_panels", "guild_id")
                            .to("guilds", "id")
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table("medals_overview_panels").if_exists().to_owned())
            .await
    }
}
