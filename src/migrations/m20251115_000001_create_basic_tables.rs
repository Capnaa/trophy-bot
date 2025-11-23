use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(Table::create()
                .table("banned_users")
                .if_not_exists()
                .col(ColumnDef::new("id").integer().not_null().auto_increment().primary_key())
                .col(ColumnDef::new("data").string().not_null())
                .to_owned())
            .await?;

        manager
            .create_table(Table::create()
                .table("guilds")
                .if_not_exists()
                .col(ColumnDef::new("id").integer().not_null().auto_increment().primary_key())
                .col(ColumnDef::new("gid").string().not_null())
                .col(ColumnDef::new("users").integer().not_null())
                .to_owned())
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop()
                .table("banned_users")
                .if_exists()
                .to_owned())
            .await?;

        manager
            .drop_table(Table::drop()
                .table("guilds")
                .if_exists()
                .to_owned())
            .await?;

        Ok(())
    }
}
