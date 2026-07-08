//! Integration tests: `Migrator::fresh` on an in-memory SQLite database, then
//! one insert+read round-trip per entity to prove the entities match the DDL,
//! plus the schema.md guarantees: ADR 0005 normalized-name uniqueness, absence
//! of a unique constraint on `(user_id, trophy_id)` (duplicates required),
//! CHECK constraints, FK `ON DELETE CASCADE`, and `down()` rollback.

use chrono::Utc;
use sea_orm::{
    ActiveModelTrait, ConnectOptions, ConnectionTrait, Database, DatabaseConnection, EntityTrait,
    NotSet, Set,
};
use sea_orm_migration::MigratorTrait;
use uuid::Uuid;

use crate::entities::{
    bot_stats, guild_settings, guilds, leaderboard_panels, role_rewards, trophies, user_trophies,
};
use crate::migrations::{run_schema_command, MigrateSubcommands, Migrator};

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

fn award_active_model(guild_id: i64, user_id: i64, trophy_id: Uuid) -> user_trophies::ActiveModel {
    user_trophies::ActiveModel {
        id: Set(Uuid::now_v7()),
        guild_id: Set(guild_id),
        user_id: Set(user_id),
        trophy_id: Set(trophy_id),
        awarded_by: Set(None),
        awarded_at: Set(now()),
        created_at: Set(now()),
        updated_at: Set(now()),
    }
}

/// Anti-regression for the schema.md / ADR 0002 requirement: **NO** unique
/// constraint on `(user_id, trophy_id)` — awarding the same trophy to the same
/// user multiple times must be accepted (F1 multi-award functionality).
#[tokio::test]
async fn duplicate_user_trophy_awards_are_accepted() {
    let db = fresh_db().await;
    let guild = insert_guild(&db, 1).await;
    let trophy = trophy_active_model(guild.id, "Repeatable", "repeatable")
        .insert(&db)
        .await
        .expect("insert trophy");

    let user_id = 42;
    for i in 1..=3 {
        award_active_model(guild.id, user_id, trophy.id)
            .insert(&db)
            .await
            .unwrap_or_else(|e| {
                panic!("award #{i} of the same (user_id, trophy_id) must be accepted: {e}")
            });
    }

    let count = user_trophies::Entity::find()
        .all(&db)
        .await
        .expect("query awards")
        .len();
    assert_eq!(count, 3, "all duplicate awards must be stored as rows");
}

#[tokio::test]
async fn trophy_value_check_constraint_enforces_range() {
    let db = fresh_db().await;
    let guild = insert_guild(&db, 1).await;

    // Boundaries are inclusive (schema.md: CHECK −999999..999999).
    for (value, name) in [(-999_999, "min"), (999_999, "max")] {
        let mut trophy = trophy_active_model(guild.id, name, name);
        trophy.value = Set(value);
        trophy
            .insert(&db)
            .await
            .unwrap_or_else(|e| panic!("boundary value {value} must be accepted: {e}"));
    }

    for (value, name) in [(-1_000_000, "toolow"), (1_000_000, "toohigh")] {
        let mut trophy = trophy_active_model(guild.id, name, name);
        trophy.value = Set(value);
        assert!(
            trophy.insert(&db).await.is_err(),
            "out-of-range value {value} must be rejected by the CHECK constraint"
        );
    }
}

#[tokio::test]
async fn role_reward_requirement_check_constraint_enforces_minimum() {
    let db = fresh_db().await;
    let guild = insert_guild(&db, 1).await;

    let reward = |role_id: i64, requirement: i32| role_rewards::ActiveModel {
        id: Set(Uuid::now_v7()),
        guild_id: Set(guild.id),
        role_id: Set(role_id),
        requirement: Set(requirement),
        created_at: Set(now()),
        updated_at: Set(now()),
    };

    reward(1, 1)
        .insert(&db)
        .await
        .expect("requirement = 1 (minimum) must be accepted");
    for requirement in [0, -5] {
        assert!(
            reward(2, requirement).insert(&db).await.is_err(),
            "requirement {requirement} must be rejected by CHECK (requirement >= 1)"
        );
    }
}

/// One mutation applied to a baseline settings model in the CHECK-range test.
type SettingsMutation = fn(&mut guild_settings::ActiveModel);

#[tokio::test]
async fn guild_settings_check_constraints_enforce_ranges() {
    let db = fresh_db().await;
    let guild = insert_guild(&db, 1).await;

    let settings = || guild_settings::ActiveModel {
        guild_id: Set(guild.id),
        dedication_display: Set(None),
        stack_roles: Set(None),
        hide_unused_trophies: Set(None),
        hide_quit_users: Set(None),
        leaderboard_format: Set(None),
        created_at: Set(now()),
        updated_at: Set(now()),
    };

    // One out-of-range probe per column (max + 1); each must be rejected.
    let out_of_range: [(&str, SettingsMutation); 6] = [
        ("dedication_display = 3", |m| m.dedication_display = Set(Some(3))),
        ("stack_roles = 2", |m| m.stack_roles = Set(Some(2))),
        ("hide_unused_trophies = 2", |m| {
            m.hide_unused_trophies = Set(Some(2))
        }),
        ("hide_quit_users = 2", |m| m.hide_quit_users = Set(Some(2))),
        ("leaderboard_format = 4", |m| m.leaderboard_format = Set(Some(4))),
        ("dedication_display = -1", |m| {
            m.dedication_display = Set(Some(-1))
        }),
    ];
    for (label, mutate) in out_of_range {
        let mut model = settings();
        mutate(&mut model);
        assert!(
            model.insert(&db).await.is_err(),
            "{label} must be rejected by its CHECK constraint"
        );
    }

    // All columns at their maximum valid value are accepted.
    let mut model = settings();
    model.dedication_display = Set(Some(2));
    model.stack_roles = Set(Some(1));
    model.hide_unused_trophies = Set(Some(1));
    model.hide_quit_users = Set(Some(1));
    model.leaderboard_format = Set(Some(3));
    model
        .insert(&db)
        .await
        .expect("maximum valid values must be accepted");
}

#[tokio::test]
async fn deleting_a_guild_cascades_to_all_child_tables() {
    let db = fresh_db().await;
    let guild = insert_guild(&db, 1).await;
    let survivor = insert_guild(&db, 2).await;

    let trophy = trophy_active_model(guild.id, "Doomed", "doomed")
        .insert(&db)
        .await
        .expect("insert trophy");
    award_active_model(guild.id, 42, trophy.id)
        .insert(&db)
        .await
        .expect("insert award");
    guild_settings::ActiveModel {
        guild_id: Set(guild.id),
        dedication_display: Set(Some(2)),
        stack_roles: Set(None),
        hide_unused_trophies: Set(None),
        hide_quit_users: Set(None),
        leaderboard_format: Set(None),
        created_at: Set(now()),
        updated_at: Set(now()),
    }
    .insert(&db)
    .await
    .expect("insert settings");
    role_rewards::ActiveModel {
        id: Set(Uuid::now_v7()),
        guild_id: Set(guild.id),
        role_id: Set(7),
        requirement: Set(10),
        created_at: Set(now()),
        updated_at: Set(now()),
    }
    .insert(&db)
    .await
    .expect("insert reward");
    leaderboard_panels::ActiveModel {
        guild_id: Set(guild.id),
        channel_id: Set(1),
        message_id: Set(2),
        created_at: Set(now()),
        updated_at: Set(now()),
    }
    .insert(&db)
    .await
    .expect("insert panel");

    let survivor_trophy = trophy_active_model(survivor.id, "Safe", "safe")
        .insert(&db)
        .await
        .expect("insert survivor trophy");

    guilds::Entity::delete_by_id(guild.id)
        .exec(&db)
        .await
        .expect("delete guild");

    assert!(trophies::Entity::find_by_id(trophy.id)
        .one(&db)
        .await
        .expect("query trophies")
        .is_none());
    assert!(user_trophies::Entity::find()
        .all(&db)
        .await
        .expect("query awards")
        .is_empty());
    assert!(guild_settings::Entity::find_by_id(guild.id)
        .one(&db)
        .await
        .expect("query settings")
        .is_none());
    assert!(role_rewards::Entity::find()
        .all(&db)
        .await
        .expect("query rewards")
        .is_empty());
    assert!(leaderboard_panels::Entity::find_by_id(guild.id)
        .one(&db)
        .await
        .expect("query panels")
        .is_none());

    // Unrelated guild's data is untouched.
    assert!(trophies::Entity::find_by_id(survivor_trophy.id)
        .one(&db)
        .await
        .expect("query survivor trophy")
        .is_some());
}

#[tokio::test]
async fn deleting_a_trophy_cascades_to_its_awards_only() {
    let db = fresh_db().await;
    let guild = insert_guild(&db, 1).await;

    let doomed = trophy_active_model(guild.id, "Doomed", "doomed")
        .insert(&db)
        .await
        .expect("insert doomed trophy");
    let kept = trophy_active_model(guild.id, "Kept", "kept")
        .insert(&db)
        .await
        .expect("insert kept trophy");
    award_active_model(guild.id, 42, doomed.id)
        .insert(&db)
        .await
        .expect("award doomed");
    let kept_award = award_active_model(guild.id, 42, kept.id)
        .insert(&db)
        .await
        .expect("award kept");

    trophies::Entity::delete_by_id(doomed.id)
        .exec(&db)
        .await
        .expect("delete trophy");

    let remaining = user_trophies::Entity::find()
        .all(&db)
        .await
        .expect("query awards");
    assert_eq!(remaining.len(), 1, "only the doomed trophy's award is gone");
    assert_eq!(remaining[0].id, kept_award.id);

    // Guild itself is untouched.
    assert!(guilds::Entity::find_by_id(guild.id)
        .one(&db)
        .await
        .expect("query guild")
        .is_some());
}

#[tokio::test]
async fn migration_down_drops_all_tables_and_up_reapplies() {
    let db = fresh_db().await;
    insert_guild(&db, 1).await;

    // Roll everything back: schema must be gone.
    Migrator::down(&db, None).await.expect("rollback migrations");
    let applied = Migrator::get_applied_migrations(&db)
        .await
        .expect("read applied migrations");
    assert!(applied.is_empty(), "no migration may remain applied");
    assert!(
        guilds::Entity::find().all(&db).await.is_err(),
        "guilds table must be dropped by down()"
    );

    // Re-apply on the same connection (refresh path): schema comes back empty.
    Migrator::up(&db, None).await.expect("reapply migrations");
    assert!(guilds::Entity::find()
        .all(&db)
        .await
        .expect("guilds table exists again")
        .is_empty());
    insert_guild(&db, 1).await;
}

/// Fix for the ops-correctness finding: schema subcommands must **propagate**
/// migration failures (non-zero exit) instead of logging and returning Ok, so
/// a cutover script chained with `&&` stops at the failed `up` rather than
/// proceeding to `import` against a broken schema.
#[tokio::test]
async fn run_schema_command_propagates_migration_failure() {
    let mut options = ConnectOptions::new("sqlite::memory:");
    options.max_connections(1).sqlx_logging(false);
    let db = Database::connect(options)
        .await
        .expect("connect to in-memory sqlite");

    // Sabotage: a pre-existing, unrecorded `guilds` table makes the initial
    // migration's CREATE TABLE fail.
    db.execute_unprepared("CREATE TABLE guilds (bogus INTEGER)")
        .await
        .expect("create conflicting table");

    for command in [
        Some(MigrateSubcommands::Up { num: None }),
        None, // bare invocation defaults to `up`
        Some(MigrateSubcommands::Refresh),
    ] {
        let label = format!("{command:?}");
        let result = run_schema_command(&db, command).await;
        assert!(
            result.is_err(),
            "{label} must return Err on a failed migration (exit non-zero), got Ok"
        );
    }
}

/// Successful schema subcommands still return Ok.
#[tokio::test]
async fn run_schema_command_returns_ok_on_success() {
    let mut options = ConnectOptions::new("sqlite::memory:");
    options.max_connections(1).sqlx_logging(false);
    let db = Database::connect(options)
        .await
        .expect("connect to in-memory sqlite");

    for command in [
        Some(MigrateSubcommands::Fresh),
        Some(MigrateSubcommands::Status),
        Some(MigrateSubcommands::Down { num: 1 }),
        Some(MigrateSubcommands::Up { num: None }),
        Some(MigrateSubcommands::Refresh),
        Some(MigrateSubcommands::Reset),
        None, // bare invocation defaults to `up`
    ] {
        let label = format!("{command:?}");
        run_schema_command(&db, command)
            .await
            .unwrap_or_else(|e| panic!("{label} must succeed: {e}"));
    }

    // The final bare invocation applied the schema.
    assert!(guilds::Entity::find()
        .all(&db)
        .await
        .expect("guilds table exists")
        .is_empty());
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
