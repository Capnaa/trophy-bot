//! Integration tests: `Migrator::fresh` on an in-memory SQLite database, then
//! one insert+read round-trip per entity to prove the entities match the DDL,
//! plus the ADR 0005 normalized-name uniqueness rules.

use chrono::Utc;
use sea_orm::{
    ActiveModelTrait, ConnectOptions, Database, DatabaseConnection, EntityTrait, NotSet, Set,
};
use sea_orm_migration::MigratorTrait;
use uuid::Uuid;

use crate::entities::{
    bot_stats, guild_settings, guilds, leaderboard_panels, role_rewards, trophies, user_trophies,
};
use crate::migrations::Migrator;

async fn fresh_db() -> DatabaseConnection {
    // A single connection is required: every pooled connection to
    // `sqlite::memory:` would otherwise get its own private database.
    let mut options = ConnectOptions::new("sqlite::memory:");
    options.max_connections(1).sqlx_logging(false);
    let db = Database::connect(options)
        .await
        .expect("connect to in-memory sqlite");
    Migrator::fresh(&db).await.expect("apply migrations");
    db
}

fn now() -> chrono::NaiveDateTime {
    Utc::now().naive_utc()
}

async fn insert_guild(db: &DatabaseConnection, id: i64) -> guilds::Model {
    guilds::ActiveModel {
        id: Set(id),
        is_safe: Set(true),
        created_at: Set(now()),
        updated_at: Set(now()),
    }
    .insert(db)
    .await
    .expect("insert guild")
}

fn trophy_active_model(guild_id: i64, name: &str, normalized_name: &str) -> trophies::ActiveModel {
    trophies::ActiveModel {
        id: Set(Uuid::now_v7()),
        guild_id: Set(guild_id),
        legacy_id: Set(Some("1".to_string())),
        creator_user_id: Set(None),
        name: Set(name.to_string()),
        normalized_name: Set(normalized_name.to_string()),
        description: Set("A trophy".to_string()),
        emoji: Set("🏆".to_string()),
        value: Set(10),
        image: Set(None),
        dedication_user_id: Set(None),
        dedication_text: Set(Some("For you".to_string())),
        details: Set("No details provided.".to_string()),
        signed: Set(false),
        created_at: Set(now()),
        updated_at: Set(now()),
    }
}

#[tokio::test]
async fn fresh_creates_schema_and_entities_round_trip() {
    let db = fresh_db().await;

    // guilds
    let guild = insert_guild(&db, 985_439_832_388_042_822).await;
    let found = guilds::Entity::find_by_id(guild.id)
        .one(&db)
        .await
        .expect("query guild")
        .expect("guild exists");
    assert_eq!(found.id, 985_439_832_388_042_822);
    assert!(found.is_safe);

    // trophies
    let trophy = trophy_active_model(guild.id, "Golden Medal", "goldenmedal")
        .insert(&db)
        .await
        .expect("insert trophy");
    let found = trophies::Entity::find_by_id(trophy.id)
        .one(&db)
        .await
        .expect("query trophy")
        .expect("trophy exists");
    assert_eq!(found.name, "Golden Medal");
    assert_eq!(found.normalized_name, "goldenmedal");
    assert_eq!(found.legacy_id.as_deref(), Some("1"));
    assert_eq!(found.creator_user_id, None);
    assert_eq!(found.emoji, "🏆");
    assert_eq!(found.value, 10);
    assert_eq!(found.dedication_text.as_deref(), Some("For you"));
    assert!(!found.signed);

    // user_trophies
    let award = user_trophies::ActiveModel {
        id: Set(Uuid::now_v7()),
        guild_id: Set(guild.id),
        user_id: Set(111_222_333_444_555_666),
        trophy_id: Set(trophy.id),
        awarded_by: Set(None), // NULL for imported legacy rows
        awarded_at: Set(now()),
        created_at: Set(now()),
        updated_at: Set(now()),
    }
    .insert(&db)
    .await
    .expect("insert user trophy");
    let found = user_trophies::Entity::find_by_id(award.id)
        .one(&db)
        .await
        .expect("query user trophy")
        .expect("award exists");
    assert_eq!(found.user_id, 111_222_333_444_555_666);
    assert_eq!(found.trophy_id, trophy.id);
    assert_eq!(found.awarded_by, None);

    // guild_settings
    guild_settings::ActiveModel {
        guild_id: Set(guild.id),
        dedication_display: Set(Some(2)),
        stack_roles: Set(None),
        hide_unused_trophies: Set(None),
        hide_quit_users: Set(Some(0)),
        leaderboard_format: Set(Some(3)),
        created_at: Set(now()),
        updated_at: Set(now()),
    }
    .insert(&db)
    .await
    .expect("insert guild settings");
    let found = guild_settings::Entity::find_by_id(guild.id)
        .one(&db)
        .await
        .expect("query settings")
        .expect("settings exist");
    assert_eq!(found.dedication_display, Some(2));
    assert_eq!(found.stack_roles, None);
    assert_eq!(found.leaderboard_format, Some(3));

    // role_rewards
    let reward = role_rewards::ActiveModel {
        id: Set(Uuid::now_v7()),
        guild_id: Set(guild.id),
        role_id: Set(777_888_999_000_111_222),
        requirement: Set(50),
        created_at: Set(now()),
        updated_at: Set(now()),
    }
    .insert(&db)
    .await
    .expect("insert role reward");
    let found = role_rewards::Entity::find_by_id(reward.id)
        .one(&db)
        .await
        .expect("query role reward")
        .expect("reward exists");
    assert_eq!(found.role_id, 777_888_999_000_111_222);
    assert_eq!(found.requirement, 50);

    // leaderboard_panels
    leaderboard_panels::ActiveModel {
        guild_id: Set(guild.id),
        channel_id: Set(631_540_341_148_876_802),
        message_id: Set(985_219_082_662_064_178),
        created_at: Set(now()),
        updated_at: Set(now()),
    }
    .insert(&db)
    .await
    .expect("insert panel");
    let found = leaderboard_panels::Entity::find_by_id(guild.id)
        .one(&db)
        .await
        .expect("query panel")
        .expect("panel exists");
    assert_eq!(found.channel_id, 631_540_341_148_876_802);
    assert_eq!(found.message_id, 985_219_082_662_064_178);

    // bot_stats (auto-increment integer PK)
    let stat = bot_stats::ActiveModel {
        id: NotSet,
        name: Set("award".to_string()),
        total: Set(41_240),
        created_at: Set(now()),
        updated_at: Set(now()),
    }
    .insert(&db)
    .await
    .expect("insert bot stat");
    let found = bot_stats::Entity::find_by_id(stat.id)
        .one(&db)
        .await
        .expect("query bot stat")
        .expect("stat exists");
    assert_eq!(found.name, "award");
    assert_eq!(found.total, 41_240);
}

#[tokio::test]
async fn normalized_name_unique_within_guild_but_not_across_guilds() {
    let db = fresh_db().await;
    let guild_a = insert_guild(&db, 1).await;
    let guild_b = insert_guild(&db, 2).await;

    trophy_active_model(guild_a.id, "Do You Smell Barbecue?", "doyousmellbarbecue")
        .insert(&db)
        .await
        .expect("first trophy inserts");

    // Same normalized name, same guild → rejected by UNIQUE(guild_id, normalized_name).
    let duplicate = trophy_active_model(guild_a.id, "do you smell barbecue", "doyousmellbarbecue")
        .insert(&db)
        .await;
    assert!(
        duplicate.is_err(),
        "duplicate normalized name within a guild must be rejected"
    );

    // Same normalized name, different guild → allowed.
    trophy_active_model(guild_b.id, "Do You Smell Barbecue?", "doyousmellbarbecue")
        .insert(&db)
        .await
        .expect("same name in another guild is allowed");
}

#[tokio::test]
async fn migration_reports_applied_status() {
    let db = fresh_db().await;
    let pending = Migrator::get_pending_migrations(&db)
        .await
        .expect("read pending migrations");
    assert!(pending.is_empty(), "all migrations must be applied");
    let applied = Migrator::get_applied_migrations(&db)
        .await
        .expect("read applied migrations");
    assert_eq!(applied.len(), 1);
}
