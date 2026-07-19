//! `/export` — normalized JSON export of a guild's data (batch C15).
//!
//! Spec: docs/specs/commands-admin.md § /export. Fixes applied:
//! - F28: the reply is EPHEMERAL (the legacy bot posted the full dump —
//!   including private trophy `details` — publicly despite the "visible only
//!   to the user" comment), and the payload is a clean, versioned export of
//!   the normalized data (guild, settings, trophies, awards, rewards, panel)
//!   instead of the raw legacy blob with internal keys.
//! - The file is built entirely in memory (`CreateAttachment::bytes`) — no
//!   temp files, so no leaks and no per-guild filename races.

use chrono::NaiveDateTime;
use poise::serenity_prelude as serenity;
use sea_orm::{ColumnTrait, ConnectionTrait, DbErr, EntityTrait, QueryFilter, QueryOrder};
use serde::Serialize;
use uuid::Uuid;

use crate::bot::{Context, Error, util};
use crate::domain::settings::{self, EffectiveSettings};
use crate::entities::{guilds, leaderboard_panels, role_rewards, trophies, user_trophies};
use crate::i18n;

/// Discriminator so consumers can recognize the file.
pub const EXPORT_FORMAT: &str = "trophy-bot-guild-export";
/// Bump when the export shape changes incompatibly.
pub const EXPORT_VERSION: u32 = 1;

/// Export the bot's data
#[poise::command(slash_command, guild_only, default_member_permissions = "ADMINISTRATOR", required_permissions = "ADMINISTRATOR")]
pub async fn export(ctx: Context<'_>) -> Result<(), Error> {
    let guild_id = util::require_guild_id(&ctx)?.get() as i64;
    let db = &ctx.data().db;
    let locale = util::locale(&ctx);

    // F28: everything about this reply is ephemeral, including the deferral.
    ctx.defer_ephemeral().await?;

    let export = build_export(db, guild_id, chrono::Utc::now().naive_utc()).await?;
    let bytes = to_json_bytes(&export)?;

    let embed = serenity::CreateEmbed::new()
        .title(i18n::t(&locale, "export-title"))
        .description(i18n::t_args(
            &locale,
            "export-description",
            &[
                ("trophies", export.trophies.len().into()),
                ("awards", export.awards.len().into()),
                ("rewards", export.rewards.len().into()),
            ],
        ))
        .colour(util::COLOR_MAIN);
    let attachment = serenity::CreateAttachment::bytes(bytes, export_filename(guild_id));
    ctx.send(
        poise::CreateReply::default()
            .embed(embed)
            .attachment(attachment)
            .ephemeral(true),
    )
    .await?;
    Ok(())
}

/// The whole export document. Discord snowflakes are serialized as strings
/// on purpose: they exceed 2^53 and would silently lose precision in any
/// JSON consumer that reads numbers as IEEE-754 doubles (e.g. JavaScript).
#[derive(Debug, Serialize)]
pub struct GuildExport {
    pub format: &'static str,
    pub version: u32,
    /// UTC.
    pub exported_at: NaiveDateTime,
    pub guild: GuildInfo,
    /// Effective settings (stored value, or the default when unset).
    pub settings: EffectiveSettings,
    pub trophies: Vec<TrophyExport>,
    pub awards: Vec<AwardExport>,
    pub rewards: Vec<RewardExport>,
    pub panel: Option<PanelExport>,
}

#[derive(Debug, Serialize)]
pub struct GuildInfo {
    pub id: String,
    pub is_safe: bool,
}

#[derive(Debug, Serialize)]
pub struct TrophyExport {
    pub id: Uuid,
    pub legacy_id: Option<String>,
    pub creator_user_id: Option<String>,
    pub name: String,
    pub description: String,
    pub emoji: String,
    pub value: i32,
    pub image: Option<String>,
    pub dedication_user_id: Option<String>,
    pub dedication_text: Option<String>,
    pub details: String,
    pub signed: bool,
    pub category: Option<String>,
    pub active: bool,
    /// UTC.
    pub created_at: NaiveDateTime,
}

impl From<trophies::Model> for TrophyExport {
    fn from(t: trophies::Model) -> Self {
        Self {
            id: t.id,
            legacy_id: t.legacy_id,
            creator_user_id: t.creator_user_id.map(|id| id.to_string()),
            name: t.name,
            description: t.description,
            emoji: t.emoji,
            value: t.value,
            image: t.image,
            dedication_user_id: t.dedication_user_id.map(|id| id.to_string()),
            dedication_text: t.dedication_text,
            details: t.details,
            signed: t.signed,
            category: t.category,
            active: t.active,
            created_at: t.created_at,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct AwardExport {
    pub id: Uuid,
    pub user_id: String,
    /// References `trophies[].id` in this same document.
    pub trophy_id: Uuid,
    /// None for imported legacy awards (never tracked).
    pub awarded_by: Option<String>,
    /// UTC; synthetic for imported legacy awards.
    pub awarded_at: NaiveDateTime,
}

impl From<user_trophies::Model> for AwardExport {
    fn from(a: user_trophies::Model) -> Self {
        Self {
            id: a.id,
            user_id: a.user_id.to_string(),
            trophy_id: a.trophy_id,
            awarded_by: a.awarded_by.map(|id| id.to_string()),
            awarded_at: a.awarded_at,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct RewardExport {
    pub role_id: String,
    pub requirement: i32,
}

impl From<role_rewards::Model> for RewardExport {
    fn from(r: role_rewards::Model) -> Self {
        Self {
            role_id: r.role_id.to_string(),
            requirement: r.requirement,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct PanelExport {
    pub channel_id: String,
    pub message_id: String,
}

impl From<leaderboard_panels::Model> for PanelExport {
    fn from(p: leaderboard_panels::Model) -> Self {
        Self {
            channel_id: p.channel_id.to_string(),
            message_id: p.message_id.to_string(),
        }
    }
}

/// Collects the guild's normalized data into an export document.
///
/// Deterministic ordering: trophies and awards by id ascending (UUIDv7 =
/// creation order), rewards by requirement then id ascending. A guild the
/// bot has never written a row for still exports cleanly (empty lists,
/// default settings, `is_safe: false`).
pub async fn build_export(
    db: &impl ConnectionTrait,
    guild_id: i64,
    exported_at: NaiveDateTime,
) -> Result<GuildExport, DbErr> {
    let guild_row = guilds::Entity::find_by_id(guild_id).one(db).await?;
    let effective = settings::effective_settings(db, guild_id).await?;
    let trophy_rows = trophies::Entity::find()
        .filter(trophies::Column::GuildId.eq(guild_id))
        .order_by_asc(trophies::Column::Id)
        .all(db)
        .await?;
    let award_rows = user_trophies::Entity::find()
        .filter(user_trophies::Column::GuildId.eq(guild_id))
        .order_by_asc(user_trophies::Column::Id)
        .all(db)
        .await?;
    let reward_rows = role_rewards::Entity::find()
        .filter(role_rewards::Column::GuildId.eq(guild_id))
        .order_by_asc(role_rewards::Column::Requirement)
        .order_by_asc(role_rewards::Column::Id)
        .all(db)
        .await?;
    let panel_row = leaderboard_panels::Entity::find_by_id(guild_id).one(db).await?;

    Ok(GuildExport {
        format: EXPORT_FORMAT,
        version: EXPORT_VERSION,
        exported_at,
        guild: GuildInfo {
            id: guild_id.to_string(),
            is_safe: guild_row.is_some_and(|g| g.is_safe),
        },
        settings: effective,
        trophies: trophy_rows.into_iter().map(Into::into).collect(),
        awards: award_rows.into_iter().map(Into::into).collect(),
        rewards: reward_rows.into_iter().map(Into::into).collect(),
        panel: panel_row.map(Into::into),
    })
}

/// Pretty-printed JSON bytes, built entirely in memory (no temp files).
pub fn to_json_bytes(export: &GuildExport) -> serde_json::Result<Vec<u8>> {
    serde_json::to_vec_pretty(export)
}

/// Attachment filename — same shape as the legacy `export-${guild}.json`.
pub fn export_filename(guild_id: i64) -> String {
    format!("export-{guild_id}.json")
}

#[cfg(test)]
mod tests {
    use sea_orm::{ActiveModelTrait, DatabaseConnection, Set};

    use super::*;
    use crate::domain::settings::Setting;
    use crate::domain::test_support::{fresh_db, insert_guild, now};
    use crate::entities::guild_settings;

    fn ts(secs: i64) -> NaiveDateTime {
        chrono::DateTime::from_timestamp(secs, 0).unwrap().naive_utc()
    }

    async fn insert_trophy(db: &DatabaseConnection, guild_id: i64, name: &str, value: i32) -> Uuid {
        let id = Uuid::now_v7();
        trophies::ActiveModel {
            id: Set(id),
            guild_id: Set(guild_id),
            legacy_id: Set(Some("7".to_string())),
            creator_user_id: Set(Some(9_007_199_254_740_993)), // > 2^53
            name: Set(name.to_string()),
            normalized_name: Set(crate::domain::normalize::normalize_name(name)),
            description: Set("A description".to_string()),
            emoji: Set("🏆".to_string()),
            value: Set(value),
            image: Set(None),
            dedication_user_id: Set(None),
            dedication_text: Set(Some("for Ana".to_string())),
            details: Set("Private details".to_string()),
            signed: Set(true),
            category: Set(Some("Government".to_string())),
            active: Set(true),
            created_at: Set(ts(1_600_000_000)),
            updated_at: Set(ts(1_600_000_000)),
        }
        .insert(db)
        .await
        .expect("insert trophy");
        id
    }

    async fn insert_award(db: &DatabaseConnection, guild_id: i64, user_id: i64, trophy_id: Uuid) {
        user_trophies::ActiveModel {
            id: Set(Uuid::now_v7()),
            guild_id: Set(guild_id),
            user_id: Set(user_id),
            trophy_id: Set(trophy_id),
            awarded_by: Set(None),
            awarded_at: Set(ts(1_600_000_100)),
            created_at: Set(now()),
            updated_at: Set(now()),
        }
        .insert(db)
        .await
        .expect("insert award");
    }

    // ---- registration -----------------------------------------------------

    #[test]
    fn export_registers_as_admin_guild_only_slash_command() {
        let command = export();
        assert_eq!(command.name, "export");
        assert!(command.guild_only);
        assert_eq!(
            command.default_member_permissions,
            poise::serenity_prelude::Permissions::ADMINISTRATOR
        );
        assert!(command.description.is_some());
    }

    // ---- pure helpers -----------------------------------------------------

    #[test]
    fn filename_matches_legacy_shape() {
        assert_eq!(export_filename(42), "export-42.json");
    }

    #[test]
    fn catalog_keys_exist() {
        let locale = i18n::resolve(None);
        assert_ne!(i18n::t(&locale, "export-title"), "export-title");
        let description: String = i18n::t_args(
            &locale,
            "export-description",
            &[
                ("trophies", 3.into()),
                ("awards", 1.into()),
                ("rewards", 0.into()),
            ],
        )
        // Fluent wraps placeables in bidi isolation marks; strip for matching.
        .chars()
        .filter(|c| !matches!(c, '\u{2068}' | '\u{2069}'))
        .collect();
        assert!(description.contains("3 trophies"), "got: {description}");
        assert!(description.contains("1 award"), "got: {description}");
        assert!(description.contains("0 role rewards"), "got: {description}");
    }

    // ---- integration (sqlite::memory:) -------------------------------------

    #[tokio::test]
    async fn empty_guild_exports_cleanly_with_defaults() {
        let db = fresh_db().await;
        // No guild row at all — the command must still export.
        let export = build_export(&db, 5, ts(0)).await.expect("build export");

        assert_eq!(export.format, EXPORT_FORMAT);
        assert_eq!(export.version, EXPORT_VERSION);
        assert_eq!(export.guild.id, "5");
        assert!(!export.guild.is_safe);
        assert!(export.trophies.is_empty());
        assert!(export.awards.is_empty());
        assert!(export.rewards.is_empty());
        assert!(export.panel.is_none());
        // Effective settings fall back to the documented defaults.
        assert_eq!(export.settings.dedication_display, Setting::DedicationDisplay.default_value());
        assert_eq!(export.settings.stack_roles, Setting::StackRoles.default_value());
        assert_eq!(
            export.settings.hide_unused_trophies,
            Setting::HideUnusedTrophies.default_value()
        );
        assert_eq!(export.settings.hide_quit_users, Setting::HideQuitUsers.default_value());
        assert_eq!(
            export.settings.leaderboard_format,
            Setting::LeaderboardFormat.default_value()
        );
    }

    #[tokio::test]
    async fn full_guild_exports_all_sections_scoped_and_ordered() {
        let db = fresh_db().await;
        insert_guild(&db, 1).await; // is_safe = true
        insert_guild(&db, 2).await;

        // Stored settings (0 differs from the default 2 and must survive).
        guild_settings::ActiveModel {
            guild_id: Set(1),
            dedication_display: Set(Some(0)),
            stack_roles: Set(None),
            hide_unused_trophies: Set(None),
            hide_quit_users: Set(Some(1)),
            leaderboard_format: Set(Some(3)),
            created_at: Set(now()),
            updated_at: Set(now()),
        }
        .insert(&db)
        .await
        .expect("insert settings");

        let gold = insert_trophy(&db, 1, "Gold", 50).await;
        let bronze = insert_trophy(&db, 1, "Bronze", 5).await;
        let foreign = insert_trophy(&db, 2, "Foreign", 1).await;
        insert_award(&db, 1, 42, gold).await;
        insert_award(&db, 1, 42, gold).await; // duplicates are real data
        insert_award(&db, 1, 43, bronze).await;
        insert_award(&db, 2, 42, foreign).await; // other guild — excluded

        role_rewards::ActiveModel {
            id: Set(Uuid::now_v7()),
            guild_id: Set(1),
            role_id: Set(111),
            requirement: Set(100),
            created_at: Set(now()),
            updated_at: Set(now()),
        }
        .insert(&db)
        .await
        .expect("insert reward 100");
        role_rewards::ActiveModel {
            id: Set(Uuid::now_v7()),
            guild_id: Set(1),
            role_id: Set(222),
            requirement: Set(10),
            created_at: Set(now()),
            updated_at: Set(now()),
        }
        .insert(&db)
        .await
        .expect("insert reward 10");

        leaderboard_panels::ActiveModel {
            guild_id: Set(1),
            channel_id: Set(333),
            message_id: Set(444),
            created_at: Set(now()),
            updated_at: Set(now()),
        }
        .insert(&db)
        .await
        .expect("insert panel");

        let export = build_export(&db, 1, ts(1_700_000_000)).await.expect("build export");

        assert!(export.guild.is_safe);
        assert_eq!(export.settings.dedication_display, 0);
        assert_eq!(export.settings.stack_roles, 1); // default filled in
        assert_eq!(export.settings.hide_quit_users, 1);
        assert_eq!(export.settings.leaderboard_format, 3);

        // Trophies: only guild 1, insertion (UUIDv7) order.
        let names: Vec<&str> = export.trophies.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, vec!["Gold", "Bronze"]);
        assert_eq!(export.trophies[0].creator_user_id.as_deref(), Some("9007199254740993"));
        assert_eq!(export.trophies[0].legacy_id.as_deref(), Some("7"));
        assert!(export.trophies[0].signed);

        // Awards: only guild 1, duplicates kept, trophy_id links resolve.
        assert_eq!(export.awards.len(), 3);
        assert!(export.awards.iter().all(|a| a.awarded_by.is_none()));
        assert_eq!(
            export.awards.iter().filter(|a| a.trophy_id == gold).count(),
            2
        );
        for award in &export.awards {
            assert!(
                export.trophies.iter().any(|t| t.id == award.trophy_id),
                "award references a trophy in the document"
            );
        }

        // Rewards ordered by requirement ascending.
        assert_eq!(
            export
                .rewards
                .iter()
                .map(|r| (r.role_id.as_str(), r.requirement))
                .collect::<Vec<_>>(),
            vec![("222", 10), ("111", 100)]
        );

        let panel = export.panel.expect("panel exported");
        assert_eq!((panel.channel_id.as_str(), panel.message_id.as_str()), ("333", "444"));
    }

    #[tokio::test]
    async fn json_output_is_versioned_and_stringifies_snowflakes() {
        let db = fresh_db().await;
        insert_guild(&db, 9_007_199_254_740_993).await; // > 2^53
        let trophy = insert_trophy(&db, 9_007_199_254_740_993, "Big", 10).await;
        insert_award(&db, 9_007_199_254_740_993, 9_007_199_254_740_995, trophy).await;

        let export = build_export(&db, 9_007_199_254_740_993, ts(1_700_000_000))
            .await
            .expect("build export");
        let bytes = to_json_bytes(&export).expect("serialize");
        let value: serde_json::Value = serde_json::from_slice(&bytes).expect("valid JSON");

        assert_eq!(value["format"], EXPORT_FORMAT);
        assert_eq!(value["version"], EXPORT_VERSION);
        assert!(value["exported_at"].is_string());
        // Snowflakes must be JSON strings — never lossy numbers.
        assert_eq!(value["guild"]["id"], "9007199254740993");
        assert_eq!(value["awards"][0]["user_id"], "9007199254740995");
        assert_eq!(value["trophies"][0]["creator_user_id"], "9007199254740993");
        // UUID link between awards and trophies survives serialization.
        assert_eq!(value["awards"][0]["trophy_id"], value["trophies"][0]["id"]);
        assert_eq!(value["panel"], serde_json::Value::Null);
    }
}
