use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(Table::create()
                .table("bot_stats")
                .if_not_exists()
                .col(ColumnDef::new("id").integer().not_null().auto_increment().primary_key())
                .col(ColumnDef::new("name").string().not_null().unique_key())
                .col(ColumnDef::new("total").big_unsigned().not_null().default(0))
                .to_owned())
            .await?;

        manager
            .create_table(Table::create()
                .table("guilds")
                .if_not_exists()
                .col(ColumnDef::new("id").string().not_null().primary_key())
                .to_owned())
            .await?;

        if let Some(legacy) = crate::legacy::LegacyData::load().await {
            let mut query = Query::insert();
            query.into_table("bot_stats")
                .columns(vec!["name", "total"]);
            for (name, total) in legacy.bot_stats() {
                query.values_panic(vec![name.into(), total.into()]);
            }
            manager.execute(query).await?;
        }

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop()
                .table("bot_stats")
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
