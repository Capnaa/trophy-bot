//! Adds cross-guild panel linking: the `guild_links` consent table plus a
//! nullable `source_guild_id` column on both panel tables. Implements the
//! `docs/specs/schema.md` additions exactly. Additive only (new table, new
//! nullable columns) so it applies cleanly to a database that already ran
//! the prior migrations — no data loss, no rewrite of earlier migrations.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        create_guild_links(manager).await?;

        manager
            .alter_table(
                Table::alter()
                    .table("leaderboard_panels")
                    .add_column(ColumnDef::new("source_guild_id").big_integer().null())
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table("active_medals_panels")
                    .add_column(ColumnDef::new("source_guild_id").big_integer().null())
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table("active_medals_panels")
                    .drop_column("source_guild_id")
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table("leaderboard_panels")
                    .drop_column("source_guild_id")
                    .to_owned(),
            )
            .await?;
        manager
            .drop_table(Table::drop().table("guild_links").if_exists().to_owned())
            .await
    }
}

fn timestamps(table: &mut TableCreateStatement) -> &mut TableCreateStatement {
    table
        .col(ColumnDef::new("created_at").timestamp().not_null())
        .col(ColumnDef::new("updated_at").timestamp().not_null())
}

async fn create_guild_links(manager: &SchemaManager<'_>) -> Result<(), DbErr> {
    manager
        .create_table(
            timestamps(
                Table::create()
                    .table("guild_links")
                    .col(ColumnDef::new("id").uuid().not_null().primary_key())
                    .col(ColumnDef::new("source_guild_id").big_integer().not_null())
                    .col(ColumnDef::new("linked_guild_id").big_integer().not_null())
                    .col(ColumnDef::new("requested_by").big_integer().not_null())
                    .col(ColumnDef::new("accepted_by").big_integer().null())
                    .col(ColumnDef::new("accepted_at").timestamp().null())
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_guild_links_source_guild")
                            .from("guild_links", "source_guild_id")
                            .to("guilds", "id")
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_guild_links_linked_guild")
                            .from("guild_links", "linked_guild_id")
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
                .name("idx_guild_links_linked_guild")
                .table("guild_links")
                .col("linked_guild_id")
                .unique()
                .to_owned(),
        )
        .await?;

    manager
        .create_index(
            Index::create()
                .name("idx_guild_links_source_guild")
                .table("guild_links")
                .col("source_guild_id")
                .to_owned(),
        )
        .await
}
