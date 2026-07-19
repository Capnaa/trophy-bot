//! Adds medal categorization + the per-category catalog panel table.
//! Implements the `docs/specs/schema.md` additions exactly. Additive only
//! (new nullable/defaulted columns, new table) so it applies cleanly to a
//! database that already ran the initial migration — no data loss, no
//! rewrite of the initial migration file.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table("trophies")
                    .add_column(ColumnDef::new("category").string_len(64).null())
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table("trophies")
                    .add_column(
                        ColumnDef::new("active")
                            .boolean()
                            .not_null()
                            .default(true),
                    )
                    .to_owned(),
            )
            .await?;

        create_active_medals_panels(manager).await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table("active_medals_panels").if_exists().to_owned())
            .await?;
        manager
            .alter_table(
                Table::alter().table("trophies").drop_column("active").to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter().table("trophies").drop_column("category").to_owned(),
            )
            .await
    }
}

fn timestamps(table: &mut TableCreateStatement) -> &mut TableCreateStatement {
    table
        .col(ColumnDef::new("created_at").timestamp().not_null())
        .col(ColumnDef::new("updated_at").timestamp().not_null())
}

async fn create_active_medals_panels(manager: &SchemaManager<'_>) -> Result<(), DbErr> {
    // One panel per category per guild, enforced by the unique index below
    // (the multi-row analogue of leaderboard_panels' single-row-per-guild PK).
    manager
        .create_table(
            timestamps(
                Table::create()
                    .table("active_medals_panels")
                    .col(ColumnDef::new("id").uuid().not_null().primary_key())
                    .col(ColumnDef::new("guild_id").big_integer().not_null())
                    .col(ColumnDef::new("category").string_len(64).not_null())
                    .col(ColumnDef::new("channel_id").big_integer().not_null())
                    .col(ColumnDef::new("message_id").big_integer().not_null())
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_active_medals_panels_guild")
                            .from("active_medals_panels", "guild_id")
                            .to("guilds", "id")
                            .on_delete(ForeignKeyAction::Cascade),
                    ),
            )
            .to_owned(),
        )
        .await?;

    manager
        .create_index(
            Index::create()
                .name("idx_active_medals_panels_guild_category")
                .table("active_medals_panels")
                .col("guild_id")
                .col("category")
                .unique()
                .to_owned(),
        )
        .await
}
