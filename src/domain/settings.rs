//! Guild settings reader (schema.md `guild_settings`): typed nullable
//! columns where NULL — or a missing row entirely — means "not explicitly
//! set" and falls back to the code-side default, mirroring the legacy
//! `getSetting` semantics (`??`, so a stored 0 is respected).

use sea_orm::{ConnectionTrait, DbErr, EntityTrait};

use crate::entities::guild_settings;

/// The five per-guild settings (core-behaviors.md / schema.md).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Setting {
    /// 0 = Always Mention, 1 = Always Name, 2 = Mention Only in Server.
    DedicationDisplay,
    /// 0 = Stack Roles, 1 = Only Highest Reward.
    StackRoles,
    /// 0 = Hide Unused Trophies, 1 = Show Unused Trophies.
    HideUnusedTrophies,
    /// 0 = Hide Quit Users, 1 = Show Quit Users.
    HideQuitUsers,
    /// 0 = Mention, 1 = Username, 2 = Nickname, 3 = Username and Tag.
    LeaderboardFormat,
}

impl Setting {
    /// Code-side default used when the column is NULL or the row is missing.
    pub const fn default_value(self) -> i16 {
        match self {
            Setting::DedicationDisplay => 2,
            Setting::StackRoles => 1,
            Setting::HideUnusedTrophies => 1,
            Setting::HideQuitUsers => 0,
            Setting::LeaderboardFormat => 0,
        }
    }

    /// The stored (explicitly set) value for this setting in a row, if any.
    fn stored(self, row: &guild_settings::Model) -> Option<i16> {
        match self {
            Setting::DedicationDisplay => row.dedication_display,
            Setting::StackRoles => row.stack_roles,
            Setting::HideUnusedTrophies => row.hide_unused_trophies,
            Setting::HideQuitUsers => row.hide_quit_users,
            Setting::LeaderboardFormat => row.leaderboard_format,
        }
    }
}

/// Effective value of one setting for a guild: the stored value if the row
/// exists and the column is non-NULL, otherwise the setting's default.
pub async fn get_setting(
    db: &impl ConnectionTrait,
    guild_id: i64,
    setting: Setting,
) -> Result<i16, DbErr> {
    let row = guild_settings::Entity::find_by_id(guild_id).one(db).await?;
    Ok(row
        .and_then(|row| setting.stored(&row))
        .unwrap_or_else(|| setting.default_value()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use sea_orm::{ActiveModelTrait, ConnectOptions, Database, DatabaseConnection, Set};
    use sea_orm_migration::MigratorTrait;

    use crate::entities::guilds;
    use crate::migrations::Migrator;

    const ALL: [Setting; 5] = [
        Setting::DedicationDisplay,
        Setting::StackRoles,
        Setting::HideUnusedTrophies,
        Setting::HideQuitUsers,
        Setting::LeaderboardFormat,
    ];

    async fn fresh_db() -> DatabaseConnection {
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

    async fn insert_guild(db: &DatabaseConnection, id: i64) {
        guilds::ActiveModel {
            id: Set(id),
            is_safe: Set(true),
            created_at: Set(now()),
            updated_at: Set(now()),
        }
        .insert(db)
        .await
        .expect("insert guild");
    }

    #[test]
    fn defaults_match_spec() {
        assert_eq!(Setting::DedicationDisplay.default_value(), 2);
        assert_eq!(Setting::StackRoles.default_value(), 1);
        assert_eq!(Setting::HideUnusedTrophies.default_value(), 1);
        assert_eq!(Setting::HideQuitUsers.default_value(), 0);
        assert_eq!(Setting::LeaderboardFormat.default_value(), 0);
    }

    #[tokio::test]
    async fn missing_row_yields_defaults() {
        let db = fresh_db().await;
        insert_guild(&db, 1).await;
        // No guild_settings row at all.
        for setting in ALL {
            let value = get_setting(&db, 1, setting).await.expect("read setting");
            assert_eq!(value, setting.default_value(), "{setting:?}");
        }
    }

    #[tokio::test]
    async fn null_columns_yield_defaults() {
        let db = fresh_db().await;
        insert_guild(&db, 1).await;
        guild_settings::ActiveModel {
            guild_id: Set(1),
            dedication_display: Set(None),
            stack_roles: Set(None),
            hide_unused_trophies: Set(None),
            hide_quit_users: Set(None),
            leaderboard_format: Set(None),
            created_at: Set(now()),
            updated_at: Set(now()),
        }
        .insert(&db)
        .await
        .expect("insert all-NULL settings row");

        for setting in ALL {
            let value = get_setting(&db, 1, setting).await.expect("read setting");
            assert_eq!(value, setting.default_value(), "{setting:?}");
        }
    }

    #[tokio::test]
    async fn stored_values_override_defaults_including_zero() {
        let db = fresh_db().await;
        insert_guild(&db, 1).await;
        guild_settings::ActiveModel {
            guild_id: Set(1),
            // 0 differs from the default (2) — a stored 0 must be respected.
            dedication_display: Set(Some(0)),
            stack_roles: Set(Some(0)),
            hide_unused_trophies: Set(Some(0)),
            hide_quit_users: Set(Some(1)),
            leaderboard_format: Set(Some(3)),
            created_at: Set(now()),
            updated_at: Set(now()),
        }
        .insert(&db)
        .await
        .expect("insert explicit settings row");

        assert_eq!(get_setting(&db, 1, Setting::DedicationDisplay).await.unwrap(), 0);
        assert_eq!(get_setting(&db, 1, Setting::StackRoles).await.unwrap(), 0);
        assert_eq!(get_setting(&db, 1, Setting::HideUnusedTrophies).await.unwrap(), 0);
        assert_eq!(get_setting(&db, 1, Setting::HideQuitUsers).await.unwrap(), 1);
        assert_eq!(get_setting(&db, 1, Setting::LeaderboardFormat).await.unwrap(), 3);
    }

    #[tokio::test]
    async fn settings_are_scoped_to_their_guild() {
        let db = fresh_db().await;
        insert_guild(&db, 1).await;
        insert_guild(&db, 2).await;
        guild_settings::ActiveModel {
            guild_id: Set(2),
            dedication_display: Set(Some(0)),
            stack_roles: Set(Some(0)),
            hide_unused_trophies: Set(None),
            hide_quit_users: Set(None),
            leaderboard_format: Set(None),
            created_at: Set(now()),
            updated_at: Set(now()),
        }
        .insert(&db)
        .await
        .expect("insert settings for guild 2");

        // Guild 1 has no row → defaults, unaffected by guild 2's row.
        assert_eq!(
            get_setting(&db, 1, Setting::StackRoles).await.unwrap(),
            Setting::StackRoles.default_value()
        );
        assert_eq!(get_setting(&db, 2, Setting::StackRoles).await.unwrap(), 0);
    }
}
