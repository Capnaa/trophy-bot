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

    /// Effective value from an optional row: stored if present, else default.
    fn resolve(self, row: Option<&guild_settings::Model>) -> i16 {
        row.and_then(|row| self.stored(row))
            .unwrap_or_else(|| self.default_value())
    }
}

/// All five effective settings for a guild, resolved with the same
/// NULL-falls-back-to-default logic as [`get_setting`]. Use this when a
/// command needs more than one setting (e.g. leaderboard needs
/// `hide_quit_users` + `leaderboard_format`) so only one row query runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EffectiveSettings {
    pub dedication_display: i16,
    pub stack_roles: i16,
    pub hide_unused_trophies: i16,
    pub hide_quit_users: i16,
    pub leaderboard_format: i16,
}

impl EffectiveSettings {
    fn from_row(row: Option<&guild_settings::Model>) -> Self {
        Self {
            dedication_display: Setting::DedicationDisplay.resolve(row),
            stack_roles: Setting::StackRoles.resolve(row),
            hide_unused_trophies: Setting::HideUnusedTrophies.resolve(row),
            hide_quit_users: Setting::HideQuitUsers.resolve(row),
            leaderboard_format: Setting::LeaderboardFormat.resolve(row),
        }
    }

    /// Field access by [`Setting`], mirroring [`get_setting`]'s shape.
    pub fn get(&self, setting: Setting) -> i16 {
        match setting {
            Setting::DedicationDisplay => self.dedication_display,
            Setting::StackRoles => self.stack_roles,
            Setting::HideUnusedTrophies => self.hide_unused_trophies,
            Setting::HideQuitUsers => self.hide_quit_users,
            Setting::LeaderboardFormat => self.leaderboard_format,
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
    Ok(setting.resolve(row.as_ref()))
}

/// All five effective settings for a guild in a single row query.
pub async fn effective_settings(
    db: &impl ConnectionTrait,
    guild_id: i64,
) -> Result<EffectiveSettings, DbErr> {
    let row = guild_settings::Entity::find_by_id(guild_id).one(db).await?;
    Ok(EffectiveSettings::from_row(row.as_ref()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use sea_orm::{ActiveModelTrait, Set};

    use crate::domain::test_support::{fresh_db, insert_guild, now};

    const ALL: [Setting; 5] = [
        Setting::DedicationDisplay,
        Setting::StackRoles,
        Setting::HideUnusedTrophies,
        Setting::HideQuitUsers,
        Setting::LeaderboardFormat,
    ];

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

    #[tokio::test]
    async fn effective_settings_missing_row_yields_all_defaults() {
        let db = fresh_db().await;
        insert_guild(&db, 1).await;

        let all = effective_settings(&db, 1).await.expect("read settings");
        assert_eq!(
            all,
            EffectiveSettings {
                dedication_display: 2,
                stack_roles: 1,
                hide_unused_trophies: 1,
                hide_quit_users: 0,
                leaderboard_format: 0,
            }
        );
    }

    #[tokio::test]
    async fn effective_settings_mixes_stored_and_defaults_per_column() {
        let db = fresh_db().await;
        insert_guild(&db, 1).await;
        guild_settings::ActiveModel {
            guild_id: Set(1),
            // Stored 0 differs from the default (2) and must be respected.
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
        .expect("insert partial settings row");

        let all = effective_settings(&db, 1).await.expect("read settings");
        assert_eq!(
            all,
            EffectiveSettings {
                dedication_display: 0,
                stack_roles: Setting::StackRoles.default_value(),
                hide_unused_trophies: Setting::HideUnusedTrophies.default_value(),
                hide_quit_users: 1,
                leaderboard_format: 3,
            }
        );
    }

    #[tokio::test]
    async fn effective_settings_agrees_with_get_setting() {
        let db = fresh_db().await;
        insert_guild(&db, 1).await;
        guild_settings::ActiveModel {
            guild_id: Set(1),
            dedication_display: Set(Some(1)),
            stack_roles: Set(Some(0)),
            hide_unused_trophies: Set(Some(0)),
            hide_quit_users: Set(None),
            leaderboard_format: Set(Some(2)),
            created_at: Set(now()),
            updated_at: Set(now()),
        }
        .insert(&db)
        .await
        .expect("insert settings row");

        let all = effective_settings(&db, 1).await.expect("read settings");
        for setting in ALL {
            assert_eq!(
                all.get(setting),
                get_setting(&db, 1, setting).await.unwrap(),
                "{setting:?}"
            );
        }
    }
}
