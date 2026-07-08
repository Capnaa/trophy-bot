//! Importer tests against a small synthetic legacy fixture (parsed with the
//! real `crate::legacy` serde model) and an in-memory SQLite target.
//! Phase rules under test: `docs/specs/migration-import.md`.

use super::report::MismatchKind;
use super::{import_data, ImportOptions, DEFAULT_DETAILS};
use crate::entities::{
    bot_stats, guild_settings, guilds, leaderboard_panels, role_rewards, trophies, user_trophies,
};
use crate::legacy::{LegacyBot, LegacyData};
use crate::migrations::Migrator;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectOptions, Database, DatabaseConnection, EntityTrait,
    NotSet, PaginatorTrait, QueryFilter, Set,
};
use sea_orm_migration::MigratorTrait;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

/// Synthetic legacy `guilds` document exercising every phase rule:
/// - guild `100`: renames (two "test"), float rounding (8.5, -2.5), orphan
///   award ("99"), duplicate award elements, reward dedupe, panel, partial
///   settings, dedications (user+name on `3`, text-only on `4`, absent on
///   `1`), incomplete trophy (`2`), score drift (user 502) and
///   rounding-induced mismatch (user 500).
/// - guild `300`: `imsafe` absent, empty settings (no row expected).
/// - `200`: `/forgetme` tombstone; `999`: corrupt non-object value.
const FIXTURE: &str = r#"{
  "100": {
    "imsafe": true,
    "settings": {"hide_quit_users": 1, "leaderboard_format": 3},
    "trophies": {
      "current": 5,
      "1": {"name": "test", "value": 10, "creator": "55", "created": 1600000000000,
            "signed": true, "details": "the details", "description": "a desc", "emoji": "🥇"},
      "2": {"name": "test", "value": 8.5},
      "3": {"name": "Unique", "value": -2.5, "creator": "55", "created": 1600000000000,
            "signed": false, "details": "d", "dedication": {"user": "42", "name": "someone"}},
      "4": {"name": "Texty", "value": 1, "creator": "55", "created": 1600000000000,
            "signed": false, "details": "d", "description": "d", "emoji": "🏆",
            "dedication": {"user": null, "name": "For the fans"}}
    },
    "users": {
      "500": {"trophies": ["1", "1", "2", "99"], "trophyValue": 28.5},
      "501": {"trophies": [], "trophyValue": 0},
      "502": {"trophies": ["1"], "trophyValue": 123}
    },
    "rewards": [
      {"role": "700", "requirement": 100},
      {"role": "700", "requirement": 50},
      {"role": "701", "requirement": 5}
    ],
    "panel": {"message": "900", "channel": "901"}
  },
  "300": {
    "settings": {},
    "trophies": {"current": 0},
    "users": {}
  },
  "200": -1,
  "999": "broken"
}"#;

async fn fresh_db() -> DatabaseConnection {
    // Single connection: each pooled connection to `sqlite::memory:` would
    // otherwise get its own private database.
    let mut options = ConnectOptions::new("sqlite::memory:");
    options.max_connections(1).sqlx_logging(false);
    let db = Database::connect(options).await.expect("connect to in-memory sqlite");
    Migrator::fresh(&db).await.expect("apply migrations");
    db
}

fn legacy_from_json(guilds_json: &str) -> LegacyData {
    LegacyData {
        bot: LegacyBot::default(),
        guilds: serde_json::from_str(guilds_json).expect("parse fixture"),
    }
}

/// Options pointing at a directory that does not exist: every local image
/// reference is "missing" and no URL download can succeed.
fn opts_no_images() -> ImportOptions {
    ImportOptions {
        images_dir: PathBuf::from("./nonexistent-images-dir-for-tests"),
        http_timeout: Duration::from_secs(1),
        ..Default::default()
    }
}

async fn import_fixture() -> (DatabaseConnection, super::ImportReport) {
    let db = fresh_db().await;
    let data = legacy_from_json(FIXTURE);
    let report = import_data(&db, &data, &opts_no_images()).await.expect("import fixture");
    (db, report)
}

async fn trophy(db: &DatabaseConnection, guild: i64, legacy_id: &str) -> trophies::Model {
    trophies::Entity::find()
        .filter(trophies::Column::GuildId.eq(guild))
        .filter(trophies::Column::LegacyId.eq(legacy_id))
        .one(db)
        .await
        .expect("query trophy")
        .unwrap_or_else(|| panic!("trophy {guild}/{legacy_id} exists"))
}

#[tokio::test]
async fn refuses_to_import_into_non_empty_target() {
    let db = fresh_db().await;
    let data = legacy_from_json(FIXTURE);
    import_data(&db, &data, &opts_no_images()).await.expect("first import into empty target");

    let err = import_data(&db, &data, &opts_no_images())
        .await
        .expect_err("second import must refuse");
    assert!(
        err.to_string().contains("refusing to import"),
        "error must clearly refuse: {err:#}"
    );

    // Nothing was duplicated by the refused run.
    let count = guilds::Entity::find().count(&db).await.expect("count guilds");
    assert_eq!(count, 2);
}

#[tokio::test]
async fn tombstones_and_corrupt_entries_are_skipped_and_reported() {
    let (db, report) = import_fixture().await;

    assert_eq!(report.tombstoned_guilds, vec!["200".to_string()]);
    // The corrupt entry carries its verbatim legacy value so the pre-cutover
    // review can inspect it without excavating the multi-megabyte blob.
    assert_eq!(report.corrupt_guilds.len(), 1);
    assert_eq!(report.corrupt_guilds[0].key, "999");
    assert_eq!(report.corrupt_guilds[0].value, serde_json::json!("broken"));
    assert_eq!(report.guilds, 2);

    let ids: Vec<i64> = guilds::Entity::find()
        .all(&db)
        .await
        .expect("query guilds")
        .into_iter()
        .map(|g| g.id)
        .collect();
    assert_eq!(ids.len(), 2);
    assert!(ids.contains(&100) && ids.contains(&300), "only valid guilds imported: {ids:?}");
}

#[tokio::test]
async fn imsafe_absent_imports_as_false() {
    let (db, _) = import_fixture().await;

    let safe = guilds::Entity::find_by_id(100).one(&db).await.unwrap().expect("guild 100");
    assert!(safe.is_safe);
    let unsafe_guild = guilds::Entity::find_by_id(300).one(&db).await.unwrap().expect("guild 300");
    assert!(!unsafe_guild.is_safe, "absent imsafe must import as false");
}

#[tokio::test]
async fn float_values_rounded_half_away_from_zero_and_reported() {
    let (db, report) = import_fixture().await;

    assert_eq!(trophy(&db, 100, "2").await.value, 9, "8.5 rounds away from zero to 9");
    assert_eq!(trophy(&db, 100, "3").await.value, -3, "-2.5 rounds away from zero to -3");
    assert_eq!(trophy(&db, 100, "1").await.value, 10, "integer values unchanged");

    assert_eq!(report.rounded_values.len(), 2);
    let by_id: HashMap<&str, (f64, i32)> = report
        .rounded_values
        .iter()
        .map(|r| (r.legacy_id.as_str(), (r.original, r.rounded)))
        .collect();
    assert_eq!(by_id.get("2"), Some(&(8.5, 9)));
    assert_eq!(by_id.get("3"), Some(&(-2.5, -3)));
}

#[tokio::test]
async fn duplicate_names_renamed_per_plan_and_reported() {
    let (db, report) = import_fixture().await;

    assert_eq!(trophy(&db, 100, "1").await.name, "test 1");
    assert_eq!(trophy(&db, 100, "2").await.name, "test 2");
    assert_eq!(trophy(&db, 100, "3").await.name, "Unique", "non-colliding names untouched");

    assert_eq!(report.renamed_trophies.len(), 2);
    for rename in &report.renamed_trophies {
        assert_eq!(rename.guild_id, 100);
        assert_eq!(rename.old_name, "test");
    }
    // The UNIQUE(guild_id, normalized_name) constraint held, so the stored
    // normalized names must be distinct.
    assert_ne!(
        trophy(&db, 100, "1").await.normalized_name,
        trophy(&db, 100, "2").await.normalized_name
    );
}

#[tokio::test]
async fn awards_one_row_per_element_with_null_awarded_by() {
    let (db, report) = import_fixture().await;

    // 500 → "1","1","2" (orphan "99" dropped); 502 → "1".
    assert_eq!(report.awards_inserted, 4);
    let all = user_trophies::Entity::find().all(&db).await.expect("query awards");
    assert_eq!(all.len(), 4);
    assert!(all.iter().all(|a| a.awarded_by.is_none()), "legacy never tracked awarded_by");

    let trophy_one = trophy(&db, 100, "1").await;
    let user_500: Vec<_> = all.iter().filter(|a| a.user_id == 500).collect();
    assert_eq!(user_500.len(), 3, "duplicates are one row each");
    assert_eq!(
        user_500.iter().filter(|a| a.trophy_id == trophy_one.id).count(),
        2,
        "the duplicated element produces two rows for the same trophy"
    );
    // Distinct UUIDv7 primary keys per row.
    let mut ids: Vec<_> = all.iter().map(|a| a.id).collect();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), 4);
}

#[tokio::test]
async fn orphan_award_elements_are_dropped_and_reported() {
    let (db, report) = import_fixture().await;

    assert_eq!(report.orphaned_awards.len(), 1);
    let orphan = &report.orphaned_awards[0];
    assert_eq!((orphan.guild_id, orphan.user_id, orphan.legacy_trophy_id.as_str()), (100, 500, "99"));

    // No award row points at a nonexistent trophy.
    let trophy_ids: Vec<_> = trophies::Entity::find()
        .all(&db)
        .await
        .expect("query trophies")
        .into_iter()
        .map(|t| t.id)
        .collect();
    for award in user_trophies::Entity::find().all(&db).await.expect("query awards") {
        assert!(trophy_ids.contains(&award.trophy_id));
    }
}

#[tokio::test]
async fn empty_award_arrays_produce_no_rows() {
    let (db, report) = import_fixture().await;

    let rows = user_trophies::Entity::find()
        .filter(user_trophies::Column::UserId.eq(501))
        .all(&db)
        .await
        .expect("query user 501 awards");
    assert!(rows.is_empty());
    assert_eq!(report.empty_award_users, 1);
    assert_eq!(report.users_with_awards, 2);
}

#[tokio::test]
async fn rewards_deduped_keeping_lowest_requirement() {
    let (db, report) = import_fixture().await;

    let rows = role_rewards::Entity::find().all(&db).await.expect("query rewards");
    let by_role: HashMap<i64, i32> = rows.iter().map(|r| (r.role_id, r.requirement)).collect();
    assert_eq!(rows.len(), 2);
    assert_eq!(by_role.get(&700), Some(&50), "lowest requirement kept for the duplicated role");
    assert_eq!(by_role.get(&701), Some(&5));

    assert_eq!(report.role_rewards, 2);
    assert_eq!(report.deduped_rewards.len(), 1);
    let removed = &report.deduped_rewards[0];
    assert_eq!(
        (removed.guild_id, removed.role_id, removed.kept_requirement, removed.removed_requirement),
        (100, 700, 50, 100)
    );
}

#[tokio::test]
async fn settings_rows_only_for_present_keys() {
    let (db, report) = import_fixture().await;

    assert_eq!(report.settings_rows, 1);
    let row = guild_settings::Entity::find_by_id(100)
        .one(&db)
        .await
        .expect("query settings")
        .expect("guild 100 has a settings row");
    assert_eq!(row.hide_quit_users, Some(1));
    assert_eq!(row.leaderboard_format, Some(3));
    assert_eq!(row.dedication_display, None, "absent keys stay NULL");
    assert_eq!(row.stack_roles, None);
    assert_eq!(row.hide_unused_trophies, None);

    let none = guild_settings::Entity::find_by_id(300).one(&db).await.expect("query 300");
    assert!(none.is_none(), "empty legacy settings map produces no row");
}

#[tokio::test]
async fn panels_imported_as_is() {
    let (db, report) = import_fixture().await;

    assert_eq!(report.panels, 1);
    let panel = leaderboard_panels::Entity::find_by_id(100)
        .one(&db)
        .await
        .expect("query panel")
        .expect("guild 100 panel");
    assert_eq!(panel.channel_id, 901);
    assert_eq!(panel.message_id, 900);
    assert!(leaderboard_panels::Entity::find_by_id(300)
        .one(&db)
        .await
        .expect("query 300 panel")
        .is_none());
}

#[tokio::test]
async fn incomplete_trophies_get_defaults_and_are_reported() {
    let (db, report) = import_fixture().await;

    let incomplete = trophy(&db, 100, "2").await;
    assert_eq!(incomplete.creator_user_id, None);
    assert!(!incomplete.signed);
    assert_eq!(incomplete.details, DEFAULT_DETAILS);

    let defaulted: Vec<&str> = report
        .defaulted_fields
        .iter()
        .filter(|d| d.legacy_id == "2")
        .map(|d| d.field)
        .collect();
    for field in ["creator", "created", "signed", "details"] {
        assert!(defaulted.contains(&field), "field `{field}` must be reported: {defaulted:?}");
    }

    // Complete trophy keeps its legacy data, including the ms timestamp.
    let complete = trophy(&db, 100, "1").await;
    assert_eq!(complete.creator_user_id, Some(55));
    assert!(complete.signed);
    assert_eq!(complete.details, "the details");
    assert_eq!(
        complete.created_at,
        chrono::DateTime::from_timestamp_millis(1_600_000_000_000).unwrap().naive_utc()
    );

    // Dedication normalization: user + name → both columns.
    let dedicated = trophy(&db, 100, "3").await;
    assert_eq!(dedicated.dedication_user_id, Some(42));
    assert_eq!(dedicated.dedication_text.as_deref(), Some("someone"));
    assert_eq!(complete.dedication_user_id, None);
    assert_eq!(complete.dedication_text, None);
}

/// Third documented legacy dedication shape (`data-model-legacy.md`, 496 in
/// production): `{"user": null, "name": "free text"}` → text only, no user.
#[tokio::test]
async fn text_only_dedication_sets_text_without_user() {
    let (db, report) = import_fixture().await;

    let texty = trophy(&db, 100, "4").await;
    assert_eq!(texty.dedication_user_id, None, "text-only dedication has no user id");
    assert_eq!(texty.dedication_text.as_deref(), Some("For the fans"));
    // A normal shape, not an anomaly: nothing reported for this trophy.
    assert!(report.defaulted_fields.iter().all(|d| d.legacy_id != "4"));
    assert!(report.invalid_fields.is_empty());
}

/// Defense paths (spec principle 3): present-but-unusable `creator`,
/// `created` and `dedication.user` are NULLed/defaulted AND reported,
/// exactly like the orphan-award defense. Production has 0 of these.
#[tokio::test]
async fn invalid_present_field_values_are_nulled_and_reported() {
    // `created` is i64::MAX ms — rejected by chrono's from_timestamp_millis.
    let fixture = r#"{
      "600": {
        "trophies": {
          "current": 1,
          "1": {"name": "Odd", "value": 1, "creator": "not-a-snowflake",
                "created": 9223372036854775807, "signed": false, "details": "d",
                "description": "d", "emoji": "🏆",
                "dedication": {"user": "someone", "name": "text"}}
        },
        "users": {}
      }
    }"#;
    let db = fresh_db().await;
    let report =
        import_data(&db, &legacy_from_json(fixture), &opts_no_images()).await.expect("import");

    let odd = trophy(&db, 600, "1").await;
    assert_eq!(odd.creator_user_id, None, "non-numeric creator → NULL");
    assert_eq!(odd.dedication_user_id, None, "non-numeric dedication user → NULL");
    assert_eq!(odd.dedication_text.as_deref(), Some("text"), "dedication text still kept");

    let invalid: HashMap<&str, &str> = report
        .invalid_fields
        .iter()
        .map(|f| {
            assert_eq!((f.guild_id, f.legacy_id.as_str()), (600, "1"));
            (f.field, f.value.as_str())
        })
        .collect();
    assert_eq!(invalid.len(), 3);
    assert_eq!(invalid.get("creator"), Some(&"not-a-snowflake"));
    assert_eq!(invalid.get("created"), Some(&"9223372036854775807"));
    assert_eq!(invalid.get("dedication.user"), Some(&"someone"));
    // Invalid values are not double-reported as absent-field defaults.
    assert!(report.defaulted_fields.is_empty());
}

/// Defense (spec principle 3): a legacy trophy value beyond the ±999,999
/// schema CHECK range is clamped to the nearest bound AND reported, instead
/// of aborting the whole all-or-nothing import with an opaque chunk-level
/// CHECK violation that names no guild or trophy. Production has 0 of these.
#[tokio::test]
async fn out_of_range_trophy_values_clamped_and_reported() {
    let fixture = r#"{
      "600": {
        "trophies": {
          "current": 2,
          "1": {"name": "TooBig", "value": 10000000, "creator": "1", "created": 1,
                "signed": false, "details": "d"},
          "2": {"name": "TooSmall", "value": -10000000.5, "creator": "1", "created": 1,
                "signed": false, "details": "d"}
        },
        "users": {"500": {"trophies": ["1"], "trophyValue": 10000000}}
      }
    }"#;
    let db = fresh_db().await;
    let report = import_data(&db, &legacy_from_json(fixture), &opts_no_images())
        .await
        .expect("out-of-range values must not abort the import");

    assert_eq!(trophy(&db, 600, "1").await.value, 999_999, "clamped to the upper CHECK bound");
    assert_eq!(trophy(&db, 600, "2").await.value, -999_999, "clamped to the lower CHECK bound");

    assert_eq!(report.invalid_fields.len(), 2);
    assert!(report.invalid_fields.iter().all(|f| f.guild_id == 600 && f.field == "value"));
    let invalid: HashMap<&str, &str> =
        report.invalid_fields.iter().map(|f| (f.legacy_id.as_str(), f.value.as_str())).collect();
    assert_eq!(invalid.get("1"), Some(&"10000000"));
    assert_eq!(invalid.get("2"), Some(&"-10000000.5"));
    // The clamp is reported as invalid, not double-reported as a rounding.
    assert!(report.rounded_values.is_empty());
    // The award of the clamped trophy still imports.
    assert_eq!(report.awards_inserted, 1);
}

/// Defense: a present setting index outside its column's CHECK range imports
/// as NULL (the code-side default applies, like an absent key) AND is
/// reported, instead of aborting on the guild_settings CHECK constraint.
#[tokio::test]
async fn out_of_range_setting_indexes_nulled_and_reported() {
    let fixture = r#"{
      "600": {
        "settings": {"stack_roles": 5, "leaderboard_format": 2},
        "trophies": {"current": 0},
        "users": {}
      },
      "601": {
        "settings": {"dedication_display": -1},
        "trophies": {"current": 0},
        "users": {}
      }
    }"#;
    let db = fresh_db().await;
    let report = import_data(&db, &legacy_from_json(fixture), &opts_no_images())
        .await
        .expect("out-of-range setting must not abort the import");

    // Guild 600 keeps a row for its valid key; the invalid one stays NULL.
    let row = guild_settings::Entity::find_by_id(600)
        .one(&db)
        .await
        .expect("query settings")
        .expect("guild 600 has a settings row");
    assert_eq!(row.stack_roles, None, "out-of-range index → NULL (code-side default)");
    assert_eq!(row.leaderboard_format, Some(2), "valid keys unaffected");

    // Guild 601's only key is invalid → no row at all, like empty settings.
    assert!(guild_settings::Entity::find_by_id(601).one(&db).await.expect("query 601").is_none());

    assert_eq!(report.invalid_fields.len(), 2);
    assert!(report.invalid_fields.iter().all(|f| f.field == "setting"));
    let invalid: HashMap<(&str, i64), &str> = report
        .invalid_fields
        .iter()
        .map(|f| ((f.legacy_id.as_str(), f.guild_id), f.value.as_str()))
        .collect();
    assert_eq!(invalid.get(&("stack_roles", 600)), Some(&"5"));
    assert_eq!(invalid.get(&("dedication_display", 601)), Some(&"-1"));
}

/// Defense: a reward entry whose requirement violates the schema
/// `CHECK (requirement >= 1)` (or exceeds i32) is dropped AND reported,
/// instead of aborting on the role_rewards CHECK constraint.
#[tokio::test]
async fn invalid_reward_requirements_dropped_and_reported() {
    let fixture = r#"{
      "600": {
        "trophies": {"current": 0},
        "users": {},
        "rewards": [
          {"role": "700", "requirement": 0},
          {"role": "701", "requirement": 5},
          {"role": "702", "requirement": 4294967296}
        ]
      }
    }"#;
    let db = fresh_db().await;
    let report = import_data(&db, &legacy_from_json(fixture), &opts_no_images())
        .await
        .expect("invalid reward requirements must not abort the import");

    let rows = role_rewards::Entity::find().all(&db).await.expect("query rewards");
    assert_eq!(rows.len(), 1, "only the valid reward is imported");
    assert_eq!((rows[0].role_id, rows[0].requirement), (701, 5));
    assert_eq!(report.role_rewards, 1);

    assert_eq!(report.invalid_fields.len(), 2);
    assert!(report.invalid_fields.iter().all(|f| f.guild_id == 600
        && f.field == "reward.requirement"));
    let invalid: HashMap<&str, &str> =
        report.invalid_fields.iter().map(|f| (f.legacy_id.as_str(), f.value.as_str())).collect();
    assert_eq!(invalid.get("700"), Some(&"0"), "below the CHECK minimum of 1");
    assert_eq!(invalid.get("702"), Some(&"4294967296"), "beyond i32");
    assert!(report.deduped_rewards.is_empty(), "dropped entries are not dedupe removals");
}

/// The emptiness check must also cover `bot_stats` — the one imported table
/// not FK-anchored to `guilds` — so a target that ran with zero guilds gets
/// the clear refusal instead of a mid-transaction UNIQUE(bot_stats.name)
/// failure.
#[tokio::test]
async fn refuses_when_only_bot_stats_rows_exist() {
    let db = fresh_db().await;
    let now = chrono::Utc::now().naive_utc();
    bot_stats::ActiveModel {
        id: NotSet,
        name: Set("total".to_owned()),
        total: Set(1),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(&db)
    .await
    .expect("seed a bot_stats row");

    let err = import_data(&db, &legacy_from_json("{}"), &opts_no_images())
        .await
        .expect_err("must refuse a target with bot_stats rows");
    assert!(err.to_string().contains("refusing to import"), "clear refusal expected: {err:#}");

    let count = bot_stats::Entity::find().count(&db).await.expect("count bot_stats");
    assert_eq!(count, 1, "the refused run must not touch existing rows");
}

/// All-or-nothing (spec principle 2): a mid-transaction insert failure must
/// roll back every insert that already ran. The trigger is two root keys
/// (`100` and `0100`) parsing to the same guild id → guilds PK violation,
/// after the bot_stats phase already inserted its rows. (Out-of-range
/// settings/values/requirements can no longer trigger this: they are caught
/// and reported during in-memory preparation instead of reaching a CHECK.)
#[tokio::test]
async fn failed_insert_rolls_back_all_prior_phases() {
    let fixture = r#"{
      "100": {
        "imsafe": true,
        "trophies": {
          "current": 1,
          "1": {"name": "T", "value": 10, "creator": "55", "created": 1600000000000,
                "signed": false, "details": "d"}
        },
        "users": {"500": {"trophies": ["1"], "trophyValue": 10}},
        "rewards": [{"role": "700", "requirement": 5}],
        "panel": {"message": "900", "channel": "901"}
      },
      "0100": {"trophies": {"current": 0}, "users": {}}
    }"#;
    let db = fresh_db().await;
    let err = import_data(&db, &legacy_from_json(fixture), &opts_no_images())
        .await
        .expect_err("duplicate guild ids must fail the import");
    assert!(err.to_string().contains("inserting guilds"), "failure is the guilds insert: {err:#}");

    // Every phase that inserted before the failure was rolled back.
    assert_eq!(bot_stats::Entity::find().count(&db).await.unwrap(), 0, "bot_stats rolled back");
    assert_eq!(guilds::Entity::find().count(&db).await.unwrap(), 0, "guilds rolled back");
    assert_eq!(trophies::Entity::find().count(&db).await.unwrap(), 0, "trophies rolled back");
    assert_eq!(user_trophies::Entity::find().count(&db).await.unwrap(), 0, "awards rolled back");
    assert_eq!(role_rewards::Entity::find().count(&db).await.unwrap(), 0, "rewards rolled back");
    assert_eq!(leaderboard_panels::Entity::find().count(&db).await.unwrap(), 0, "panels rolled back");
    assert_eq!(guild_settings::Entity::find().count(&db).await.unwrap(), 0, "settings rolled back");
}

#[tokio::test]
async fn score_mismatches_classified_as_rounding_or_legacy_drift() {
    let (_db, report) = import_fixture().await;

    assert_eq!(report.score_mismatches.len(), 2);
    let by_user: HashMap<i64, &super::report::ScoreMismatch> =
        report.score_mismatches.iter().map(|m| (m.user_id, m)).collect();

    // User 500: stored 28.5 == raw sum (10+10+8.5) but != rounded sum 29.
    let rounding = by_user.get(&500).expect("user 500 mismatch");
    assert_eq!(rounding.kind, MismatchKind::Rounding);
    assert_eq!(rounding.recalculated, 29);
    assert_eq!(rounding.stored, 28.5);

    // User 502: stored 123 vs raw/rounded 10 — genuine legacy drift.
    let drift = by_user.get(&502).expect("user 502 mismatch");
    assert_eq!(drift.kind, MismatchKind::LegacyDrift);
    assert_eq!(drift.recalculated, 10);

    // User 501 (0 == 0) is not a mismatch — validated by len() == 2 above.
}

#[tokio::test]
async fn bot_stats_imported_as_historical_record() {
    let db = fresh_db().await;
    let bot: LegacyBot = serde_json::from_str(
        r#"{"commands":{"total":10,"award":7},"trophies":3,"trophiesAwarded":5}"#,
    )
    .expect("parse bot fixture");
    let data = LegacyData { bot, guilds: serde_json::from_str("{}").expect("empty guilds") };

    let report = import_data(&db, &data, &opts_no_images()).await.expect("import");
    assert_eq!(report.bot_stats_rows, 4);

    let rows: HashMap<String, i64> = bot_stats::Entity::find()
        .all(&db)
        .await
        .expect("query bot stats")
        .into_iter()
        .map(|r| (r.name, r.total))
        .collect();
    assert_eq!(rows.get("total"), Some(&10));
    assert_eq!(rows.get("award"), Some(&7));
    assert_eq!(rows.get("trophiesAwarded"), Some(&5));
    assert_eq!(rows.get("rootTrophies"), Some(&3));
}

const IMAGE_FIXTURE: &str = r#"{
  "400": {
    "trophies": {
      "current": 4,
      "1": {"name": "HasFile", "value": 1, "creator": "1", "created": 1, "signed": false,
            "details": "d", "image": "400_1.png"},
      "2": {"name": "MissingFile", "value": 1, "creator": "1", "created": 1, "signed": false,
            "details": "d", "image": "400_2.png"},
      "3": {"name": "Remote", "value": 1, "creator": "1", "created": 1, "signed": false,
            "details": "d", "image": "http://127.0.0.1:9/pic.gif?ex=deadbeef"}
    },
    "users": {}
  }
}"#;

fn temp_images_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("trophy-import-test-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create temp images dir");
    dir
}

#[tokio::test]
async fn images_local_kept_missing_nulled_urls_expired_orphans_listed() {
    let dir = temp_images_dir("images");
    std::fs::write(dir.join("400_1.png"), b"png").expect("write referenced file");
    std::fs::write(dir.join("orphan.png"), b"png").expect("write orphan file");

    let db = fresh_db().await;
    let data = legacy_from_json(IMAGE_FIXTURE);
    let opts = ImportOptions {
        images_dir: dir.clone(),
        http_timeout: Duration::from_secs(1),
        ..Default::default()
    };
    let report = import_data(&db, &data, &opts).await.expect("import never fails on images");

    assert_eq!(trophy(&db, 400, "1").await.image.as_deref(), Some("400_1.png"));
    assert_eq!(trophy(&db, 400, "2").await.image, None, "missing file → NULL");
    assert_eq!(trophy(&db, 400, "3").await.image, None, "dead URL → NULL");

    assert_eq!(report.local_images_kept, 1);
    assert_eq!(report.missing_image_files.len(), 1);
    assert_eq!(report.missing_image_files[0].filename, "400_2.png");
    assert_eq!(report.expired_image_urls.len(), 1);
    assert!(report.expired_image_urls[0].url.starts_with("http://127.0.0.1:9/"));
    assert!(report.downloaded_images.is_empty());
    assert_eq!(report.url_images(), 1);
    assert_eq!(report.orphan_disk_files, vec!["orphan.png".to_string()]);

    let _ = std::fs::remove_dir_all(&dir);
}

/// Minimal local HTTP server: answers every connection with `200 OK` + `body`.
async fn serve_images(listener: tokio::net::TcpListener, body: &'static [u8]) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    while let Ok((mut sock, _)) = listener.accept().await {
        tokio::spawn(async move {
            let mut buf = [0u8; 2048];
            let _ = sock.read(&mut buf).await; // request headers; content ignored
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = sock.write_all(head.as_bytes()).await;
            let _ = sock.write_all(body).await;
            let _ = sock.shutdown().await;
        });
    }
}

/// Phase 6 success path: a live CDN URL is downloaded to the images dir as
/// `{guild}_{legacy_id}.{ext}`, the trophy stores that filename, and the
/// download is reported.
#[tokio::test]
async fn url_images_downloaded_saved_and_reported() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind local server");
    let port = listener.local_addr().expect("local addr").port();
    tokio::spawn(serve_images(listener, b"GIFDATA"));

    let dir = temp_images_dir("download");
    let fixture = format!(
        r#"{{
          "400": {{
            "trophies": {{
              "current": 1,
              "1": {{"name": "Remote", "value": 1, "creator": "1", "created": 1, "signed": false,
                    "details": "d", "image": "http://127.0.0.1:{port}/pic.gif?ex=deadbeef"}}
            }},
            "users": {{}}
          }}
        }}"#
    );
    let db = fresh_db().await;
    let opts = ImportOptions {
        images_dir: dir.clone(),
        http_timeout: Duration::from_secs(5),
        ..Default::default()
    };
    let report = import_data(&db, &legacy_from_json(&fixture), &opts).await.expect("import");

    assert_eq!(trophy(&db, 400, "1").await.image.as_deref(), Some("400_1.gif"));
    assert_eq!(
        std::fs::read(dir.join("400_1.gif")).expect("downloaded file exists"),
        b"GIFDATA",
        "downloaded bytes written to the images dir"
    );

    assert_eq!(report.downloaded_images.len(), 1);
    let downloaded = &report.downloaded_images[0];
    assert_eq!(
        (downloaded.guild_id, downloaded.legacy_id.as_str(), downloaded.filename.as_str()),
        (400, "1", "400_1.gif")
    );
    assert!(downloaded.url.starts_with("http://127.0.0.1:"));
    assert!(report.expired_image_urls.is_empty());
    assert_eq!(report.url_images(), 1);
    assert!(
        report.orphan_disk_files.is_empty(),
        "the downloaded file is referenced, not an orphan: {:?}",
        report.orphan_disk_files
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn report_serializes_to_json_with_summary() {
    let (_db, report) = import_fixture().await;

    let json = serde_json::to_value(&report).expect("report serializes");
    for key in [
        "guilds",
        "tombstoned_guilds",
        "corrupt_guilds",
        "trophies",
        "defaulted_fields",
        "invalid_fields",
        "rounded_values",
        "renamed_trophies",
        "awards_inserted",
        "orphaned_awards",
        "deduped_rewards",
        "panels",
        "settings_rows",
        "missing_image_files",
        "expired_image_urls",
        "downloaded_images",
        "orphan_disk_files",
        "score_mismatches",
    ] {
        assert!(json.get(key).is_some(), "report JSON must contain `{key}`");
    }

    // Summary compares measured vs the production-expected counts.
    let rows = report.summary_rows();
    let row = |name: &str| -> (u64, u64) {
        let (_, measured, expected) =
            rows.iter().find(|(n, _, _)| *n == name).unwrap_or_else(|| panic!("`{name}` row"));
        (*measured, *expected)
    };
    assert_eq!(row("guilds"), (2, 2_488));

    // Every spec-stated production count is machine-checked in the summary
    // (migration-import.md), not just present as a raw JSON field:
    // 43 incomplete trophies (Phase 3), 1,284 empty-award users (Phase 4),
    // 162 guilds with non-empty settings (Phase 5), 2,693 − 200 = 2,493 local
    // images kept and 278 orphan disk files (Phase 6).
    assert_eq!(
        row("defaulted_trophies"),
        (1, 43),
        "only fixture trophy 100/2 misses CORE fields (creator/created/signed); 100/3 misses \
         only description+emoji, which never count toward the spec's 43 incomplete trophies"
    );
    assert_eq!(
        row("defaulted_details"),
        (1, 360),
        "fixture trophy 100/2 also misses `details` (expected legacy shape, tracked separately)"
    );
    assert_eq!(row("empty_award_users"), (1, 1_284), "fixture user 501 has an empty array");
    assert_eq!(row("settings_rows"), (1, 162), "guild 100 only; empty settings get no row");
    assert_eq!(row("local_images_kept"), (0, 2_493), "no images dir in this fixture");
    assert_eq!(row("orphan_disk_files"), (0, 278), "no images dir in this fixture");
}

/// The spec's expected 43 counts incomplete TROPHIES (distinct per guild,
/// missing a CORE field: creator/created/signed), while `defaulted_fields`
/// records one entry per absent FIELD. `details`-only defaults are expected
/// legacy shape and tracked in their own metric (expected 360).
#[test]
fn defaulted_trophies_counts_distinct_core_incomplete_trophies() {
    use super::report::{DefaultedField, ImportReport};

    let mut report = ImportReport::default();
    for (guild_id, legacy_id, field) in [
        (100, "2", "creator"),
        (100, "2", "created"),
        (100, "2", "signed"),
        (100, "7", "details"), // details-only: excluded from the 43-metric
        (300, "2", "creator"), // same legacy id, different guild
    ] {
        report.defaulted_fields.push(DefaultedField {
            guild_id,
            legacy_id: legacy_id.to_string(),
            field,
        });
    }

    assert_eq!(report.defaulted_trophies(), 2, "100/2 and 300/2 miss core fields; 100/7 not");
    assert_eq!(report.defaulted_details(), 1, "only 100/7 misses details");
    let rows = report.summary_rows();
    let (_, measured, expected) =
        rows.iter().find(|(n, _, _)| *n == "defaulted_trophies").expect("defaulted_trophies row");
    assert_eq!((*measured, *expected), (2, 43));
}
