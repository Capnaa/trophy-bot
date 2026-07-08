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
    ColumnTrait, ConnectOptions, Database, DatabaseConnection, EntityTrait, PaginatorTrait,
    QueryFilter,
};
use sea_orm_migration::MigratorTrait;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

/// Synthetic legacy `guilds` document exercising every phase rule:
/// - guild `100`: renames (two "test"), float rounding (8.5, -2.5), orphan
///   award ("99"), duplicate award elements, reward dedupe, panel, partial
///   settings, dedications, incomplete trophy (`2`), score drift (user 502)
///   and rounding-induced mismatch (user 500).
/// - guild `300`: `imsafe` absent, empty settings (no row expected).
/// - `200`: `/forgetme` tombstone; `999`: corrupt non-object value.
const FIXTURE: &str = r#"{
  "100": {
    "imsafe": true,
    "settings": {"hide_quit_users": 1, "leaderboard_format": 3},
    "trophies": {
      "current": 4,
      "1": {"name": "test", "value": 10, "creator": "55", "created": 1600000000000,
            "signed": true, "details": "the details", "description": "a desc", "emoji": "🥇"},
      "2": {"name": "test", "value": 8.5},
      "3": {"name": "Unique", "value": -2.5, "creator": "55", "created": 1600000000000,
            "signed": false, "details": "d", "dedication": {"user": "42", "name": "someone"}}
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
    assert_eq!(report.corrupt_guilds, vec!["999".to_string()]);
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
    let guilds_row = rows.iter().find(|(name, _, _)| *name == "guilds").expect("guilds row");
    assert_eq!(guilds_row.1, 2, "measured");
    assert_eq!(guilds_row.2, 2_488, "expected (production)");
}
