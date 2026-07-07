//! Initial normalized schema — implements `docs/specs/schema.md` exactly.
//!
//! Seven tables: guilds, trophies, user_trophies, guild_settings, role_rewards,
//! leaderboard_panels, bot_stats. No legacy-data inserts (ADR 0008 — the
//! importer owns data). Portable across SQLite/PostgreSQL (ADR 0003): UUIDs and
//! timestamps are app-generated, no triggers, no engine-specific SQL.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        create_guilds(manager).await?;
        create_trophies(manager).await?;
        create_user_trophies(manager).await?;
        create_guild_settings(manager).await?;
        create_role_rewards(manager).await?;
        create_leaderboard_panels(manager).await?;
        create_bot_stats(manager).await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // Reverse dependency order: children before guilds.
        for table in [
            "bot_stats",
            "leaderboard_panels",
            "role_rewards",
            "guild_settings",
            "user_trophies",
            "trophies",
            "guilds",
        ] {
            manager
                .drop_table(Table::drop().table(table).if_exists().to_owned())
                .await?;
        }
        Ok(())
    }
}

/// `created_at` / `updated_at`, NOT NULL, app-maintained (no DB defaults or
/// triggers — schema.md conventions).
fn timestamps(table: &mut TableCreateStatement) -> &mut TableCreateStatement {
    table
        .col(ColumnDef::new("created_at").timestamp().not_null())
        .col(ColumnDef::new("updated_at").timestamp().not_null())
}

fn guild_fk(from_table: &'static str) -> ForeignKeyCreateStatement {
    ForeignKey::create()
        .name(format!("fk_{from_table}_guild"))
        .from(from_table, "guild_id")
        .to("guilds", "id")
        .on_delete(ForeignKeyAction::Cascade)
        .to_owned()
}

async fn create_guilds(manager: &SchemaManager<'_>) -> Result<(), DbErr> {
    manager
        .create_table(
            timestamps(
                Table::create()
                    .table("guilds")
                    .col(ColumnDef::new("id").big_integer().not_null().primary_key())
                    .col(
                        ColumnDef::new("is_safe")
                            .boolean()
                            .not_null()
                            .default(false),
                    ),
            )
            .to_owned(),
        )
        .await
}

async fn create_trophies(manager: &SchemaManager<'_>) -> Result<(), DbErr> {
    manager
        .create_table(
            timestamps(
                Table::create()
                    .table("trophies")
                    .col(ColumnDef::new("id").uuid().not_null().primary_key())
                    .col(ColumnDef::new("guild_id").big_integer().not_null())
                    .col(ColumnDef::new("legacy_id").text().null())
                    .col(ColumnDef::new("creator_user_id").big_integer().null())
                    .col(ColumnDef::new("name").string_len(32).not_null())
                    .col(ColumnDef::new("normalized_name").string_len(64).not_null())
                    .col(
                        ColumnDef::new("description")
                            .string_len(128)
                            .not_null()
                            .default("No description provided"),
                    )
                    .col(
                        ColumnDef::new("emoji")
                            .string_len(64)
                            .not_null()
                            .default("🏆"),
                    )
                    .col(
                        ColumnDef::new("value")
                            .integer()
                            .not_null()
                            .check(Expr::col("value").between(-999_999, 999_999)),
                    )
                    .col(ColumnDef::new("image").string_len(255).null())
                    .col(ColumnDef::new("dedication_user_id").big_integer().null())
                    .col(ColumnDef::new("dedication_text").string_len(32).null())
                    .col(
                        ColumnDef::new("details")
                            .string_len(300)
                            .not_null()
                            .default("No details provided."),
                    )
                    .col(
                        ColumnDef::new("signed")
                            .boolean()
                            .not_null()
                            .default(false),
                    )
                    .foreign_key(&mut guild_fk("trophies")),
            )
            .to_owned(),
        )
        .await?;

    // ADR 0005: case/punctuation-insensitive uniqueness within a guild.
    manager
        .create_index(
            Index::create()
                .name("idx_trophies_guild_normalized_name")
                .table("trophies")
                .col("guild_id")
                .col("normalized_name")
                .unique()
                .to_owned(),
        )
        .await?;

    manager
        .create_index(
            Index::create()
                .name("idx_trophies_guild")
                .table("trophies")
                .col("guild_id")
                .to_owned(),
        )
        .await
}

async fn create_user_trophies(manager: &SchemaManager<'_>) -> Result<(), DbErr> {
    manager
        .create_table(
            timestamps(
                Table::create()
                    .table("user_trophies")
                    .col(ColumnDef::new("id").uuid().not_null().primary_key())
                    .col(ColumnDef::new("guild_id").big_integer().not_null())
                    .col(ColumnDef::new("user_id").big_integer().not_null())
                    .col(ColumnDef::new("trophy_id").uuid().not_null())
                    // NULL for all imported legacy rows — never NOT NULL.
                    .col(ColumnDef::new("awarded_by").big_integer().null())
                    .col(ColumnDef::new("awarded_at").timestamp().not_null())
                    .foreign_key(&mut guild_fk("user_trophies"))
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_user_trophies_trophy")
                            .from("user_trophies", "trophy_id")
                            .to("trophies", "id")
                            .on_delete(ForeignKeyAction::Cascade),
                    ),
                // NO unique constraint on (user_id, trophy_id): duplicates are
                // required functionality (ADR 0002).
            )
            .to_owned(),
        )
        .await?;

    manager
        .create_index(
            Index::create()
                .name("idx_user_trophies_guild_user")
                .table("user_trophies")
                .col("guild_id")
                .col("user_id")
                .to_owned(),
        )
        .await?;

    manager
        .create_index(
            Index::create()
                .name("idx_user_trophies_trophy")
                .table("user_trophies")
                .col("trophy_id")
                .to_owned(),
        )
        .await?;

    manager
        .create_index(
            Index::create()
                .name("idx_user_trophies_revoke_path")
                .table("user_trophies")
                .col("guild_id")
                .col("user_id")
                .col("trophy_id")
                .col("awarded_at")
                .to_owned(),
        )
        .await
}

async fn create_guild_settings(manager: &SchemaManager<'_>) -> Result<(), DbErr> {
    // NULL = "not explicitly set" → code-side default (legacy getSetting parity).
    manager
        .create_table(
            timestamps(
                Table::create()
                    .table("guild_settings")
                    .col(
                        ColumnDef::new("guild_id")
                            .big_integer()
                            .not_null()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new("dedication_display")
                            .small_integer()
                            .null()
                            .check(Expr::col("dedication_display").between(0, 2)),
                    )
                    .col(
                        ColumnDef::new("stack_roles")
                            .small_integer()
                            .null()
                            .check(Expr::col("stack_roles").between(0, 1)),
                    )
                    .col(
                        ColumnDef::new("hide_unused_trophies")
                            .small_integer()
                            .null()
                            .check(Expr::col("hide_unused_trophies").between(0, 1)),
                    )
                    .col(
                        ColumnDef::new("hide_quit_users")
                            .small_integer()
                            .null()
                            .check(Expr::col("hide_quit_users").between(0, 1)),
                    )
                    .col(
                        ColumnDef::new("leaderboard_format")
                            .small_integer()
                            .null()
                            .check(Expr::col("leaderboard_format").between(0, 3)),
                    )
                    .foreign_key(&mut guild_fk("guild_settings")),
            )
            .to_owned(),
        )
        .await
}

async fn create_role_rewards(manager: &SchemaManager<'_>) -> Result<(), DbErr> {
    manager
        .create_table(
            timestamps(
                Table::create()
                    .table("role_rewards")
                    .col(ColumnDef::new("id").uuid().not_null().primary_key())
                    .col(ColumnDef::new("guild_id").big_integer().not_null())
                    .col(ColumnDef::new("role_id").big_integer().not_null())
                    .col(
                        ColumnDef::new("requirement")
                            .integer()
                            .not_null()
                            .check(Expr::col("requirement").gte(1)),
                    )
                    .foreign_key(&mut guild_fk("role_rewards")),
            )
            .to_owned(),
        )
        .await?;

    manager
        .create_index(
            Index::create()
                .name("idx_role_rewards_guild_role")
                .table("role_rewards")
                .col("guild_id")
                .col("role_id")
                .unique()
                .to_owned(),
        )
        .await
}

async fn create_leaderboard_panels(manager: &SchemaManager<'_>) -> Result<(), DbErr> {
    // One panel per guild, enforced by the PK (F30).
    manager
        .create_table(
            timestamps(
                Table::create()
                    .table("leaderboard_panels")
                    .col(
                        ColumnDef::new("guild_id")
                            .big_integer()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new("channel_id").big_integer().not_null())
                    .col(ColumnDef::new("message_id").big_integer().not_null())
                    .foreign_key(&mut guild_fk("leaderboard_panels")),
            )
            .to_owned(),
        )
        .await
}

async fn create_bot_stats(manager: &SchemaManager<'_>) -> Result<(), DbErr> {
    manager
        .create_table(
            timestamps(
                Table::create()
                    .table("bot_stats")
                    .col(
                        ColumnDef::new("id")
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(ColumnDef::new("name").string().not_null().unique_key())
                    .col(ColumnDef::new("total").big_integer().not_null().default(0)),
            )
            .to_owned(),
        )
        .await
}
