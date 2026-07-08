//! `/settings list` + `/settings set <setting> [value]` (batch C11).
//!
//! Spec: docs/specs/commands-admin.md §/settings. Fixes applied:
//! - F26: no free-text parsing. `set` is a subcommand group with one
//!   subcommand per setting, each exposing native Discord choices for its
//!   value — the 1-based-number and substring-matching quirks are gone.
//! - F27: settings and values are typed enums end to end; unknown setting
//!   ids are unrepresentable and out-of-range indexes are rejected (write
//!   path) or rendered as the raw number (read path) instead of crashing.
//!
//! Parity kept: omitting `value` resets the setting to its default (the
//! default index is written explicitly, like the legacy bot did); `list`
//! shows all five settings with the stored-or-default option label, the
//! description and every option; the confirmation/list wording matches
//! settings.js.

use poise::serenity_prelude as serenity;
use sea_orm::sea_query::OnConflict;
use sea_orm::{DatabaseConnection, EntityTrait, Set, TransactionTrait};

use crate::bot::{Context, Error, util};
use crate::domain::settings::{EffectiveSettings, Setting, effective_settings};
use crate::entities::{guild_settings, guilds};
use crate::i18n::{self, LanguageIdentifier};

/// The five settings in the legacy display order (globals.js `settings`).
pub(crate) const ALL_SETTINGS: [Setting; 5] = [
    Setting::DedicationDisplay,
    Setting::StackRoles,
    Setting::HideUnusedTrophies,
    Setting::HideQuitUsers,
    Setting::LeaderboardFormat,
];

// ---------------------------------------------------------------------------
// Command definitions
// ---------------------------------------------------------------------------

/// Modify the server settings for the bot.
#[poise::command(
    slash_command,
    guild_only,
    default_member_permissions = "MANAGE_GUILD",
    required_permissions = "MANAGE_GUILD",
    subcommands("set", "list"),
    subcommand_required
)]
pub async fn settings(_ctx: Context<'_>) -> Result<(), Error> {
    // Never reached: subcommand_required.
    Ok(())
}

/// Change a setting for the server.
#[poise::command(
    slash_command,
    guild_only,
    subcommands(
        "dedication_display",
        "stack_roles",
        "hide_unused_trophies",
        "hide_quit_users",
        "leaderboard_format"
    ),
    subcommand_required
)]
async fn set(_ctx: Context<'_>) -> Result<(), Error> {
    // Never reached: subcommand_required.
    Ok(())
}

/// How to display the dedication of a trophy when it is given.
#[poise::command(slash_command, guild_only)]
async fn dedication_display(
    ctx: Context<'_>,
    #[description = "The new value. Omit to reset to the default."] value: Option<
        DedicationDisplayChoice,
    >,
) -> Result<(), Error> {
    apply_set(ctx, Setting::DedicationDisplay, value.map(|v| v as i16)).await
}

/// Whether role rewards stack or only the highest reward is kept.
#[poise::command(slash_command, guild_only)]
async fn stack_roles(
    ctx: Context<'_>,
    #[description = "The new value. Omit to reset to the default."] value: Option<
        StackRolesChoice,
    >,
) -> Result<(), Error> {
    apply_set(ctx, Setting::StackRoles, value.map(|v| v as i16)).await
}

/// Whether trophies nobody holds are hidden from regular users.
#[poise::command(slash_command, guild_only)]
async fn hide_unused_trophies(
    ctx: Context<'_>,
    #[description = "The new value. Omit to reset to the default."] value: Option<
        HideUnusedTrophiesChoice,
    >,
) -> Result<(), Error> {
    apply_set(ctx, Setting::HideUnusedTrophies, value.map(|v| v as i16)).await
}

/// Whether users that left the server are hidden from the leaderboard.
#[poise::command(slash_command, guild_only)]
async fn hide_quit_users(
    ctx: Context<'_>,
    #[description = "The new value. Omit to reset to the default."] value: Option<
        HideQuitUsersChoice,
    >,
) -> Result<(), Error> {
    apply_set(ctx, Setting::HideQuitUsers, value.map(|v| v as i16)).await
}

/// How to display users on the leaderboard.
#[poise::command(slash_command, guild_only)]
async fn leaderboard_format(
    ctx: Context<'_>,
    #[description = "The new value. Omit to reset to the default."] value: Option<
        LeaderboardFormatChoice,
    >,
) -> Result<(), Error> {
    apply_set(ctx, Setting::LeaderboardFormat, value.map(|v| v as i16)).await
}

/// List all settings of the server
#[poise::command(slash_command, guild_only)]
async fn list(ctx: Context<'_>) -> Result<(), Error> {
    let guild_id = require_guild_id(&ctx)?;
    // Clone the name out before any await: GuildRef borrows the cache.
    let guild_name = ctx.guild().map(|guild| guild.name.clone());
    let locale = util::locale(&ctx);

    let effective = effective_settings(&ctx.data().db, guild_id).await?;

    let guild_name =
        guild_name.unwrap_or_else(|| i18n::t(&locale, "settings-list-fallback-guild-name"));
    let embed = serenity::CreateEmbed::new()
        .title(i18n::t_args(
            &locale,
            "settings-list-title",
            &[("guild", guild_name.into())],
        ))
        .description(render_list_body(&locale, &effective))
        .footer(serenity::CreateEmbedFooter::new(i18n::t(
            &locale,
            "settings-list-footer",
        )))
        .colour(util::COLOR_MAIN);

    util::reply_embed(ctx, embed, false).await
}

// ---------------------------------------------------------------------------
// Value choices (F26) — one native-choice enum per setting; the variant
// order is the stored 0-based index, matching `Setting`'s documentation.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, poise::ChoiceParameter)]
pub(crate) enum DedicationDisplayChoice {
    #[name = "Always Mention"]
    AlwaysMention,
    #[name = "Always Name"]
    AlwaysName,
    #[name = "Mention Only in Server"]
    MentionOnlyInServer,
}

#[derive(Debug, Clone, Copy, poise::ChoiceParameter)]
pub(crate) enum StackRolesChoice {
    #[name = "Stack Roles"]
    StackRoles,
    #[name = "Only Highest Reward"]
    OnlyHighestReward,
}

#[derive(Debug, Clone, Copy, poise::ChoiceParameter)]
pub(crate) enum HideUnusedTrophiesChoice {
    #[name = "Hide Unused Trophies"]
    HideUnusedTrophies,
    #[name = "Show Unused Trophies"]
    ShowUnusedTrophies,
}

#[derive(Debug, Clone, Copy, poise::ChoiceParameter)]
pub(crate) enum HideQuitUsersChoice {
    #[name = "Hide Quit Users"]
    HideQuitUsers,
    #[name = "Show Quit Users"]
    ShowQuitUsers,
}

#[derive(Debug, Clone, Copy, poise::ChoiceParameter)]
pub(crate) enum LeaderboardFormatChoice {
    #[name = "Mention"]
    Mention,
    #[name = "Username"]
    Username,
    #[name = "Nickname"]
    Nickname,
    #[name = "Username and Tag"]
    UsernameAndTag,
}

// ---------------------------------------------------------------------------
// Business logic (testable, no Discord types)
// ---------------------------------------------------------------------------

/// Stable id used to build the Fluent keys for a setting.
pub(crate) const fn setting_key(setting: Setting) -> &'static str {
    match setting {
        Setting::DedicationDisplay => "dedication-display",
        Setting::StackRoles => "stack-roles",
        Setting::HideUnusedTrophies => "hide-unused-trophies",
        Setting::HideQuitUsers => "hide-quit-users",
        Setting::LeaderboardFormat => "leaderboard-format",
    }
}

/// Number of options a setting has (indexes `0..count` are valid).
pub(crate) const fn option_count(setting: Setting) -> i16 {
    match setting {
        Setting::DedicationDisplay => 3,
        Setting::StackRoles => 2,
        Setting::HideUnusedTrophies => 2,
        Setting::HideQuitUsers => 2,
        Setting::LeaderboardFormat => 4,
    }
}

/// The index to store: the chosen one, or the setting's default when the
/// value was omitted ("omit value = reset to default", spec Rust target).
pub(crate) fn effective_index(setting: Setting, chosen: Option<i16>) -> i16 {
    chosen.unwrap_or_else(|| setting.default_value())
}

/// Localized display name of a setting.
pub(crate) fn setting_name(locale: &LanguageIdentifier, setting: Setting) -> String {
    i18n::t(locale, &format!("settings-{}-name", setting_key(setting)))
}

/// Localized description of a setting.
pub(crate) fn setting_description(locale: &LanguageIdentifier, setting: Setting) -> String {
    i18n::t(locale, &format!("settings-{}-description", setting_key(setting)))
}

/// Localized label of one option. Out-of-range indexes (corrupt data) fall
/// back to the raw number instead of crashing (F27 — no crash paths).
pub(crate) fn option_label(locale: &LanguageIdentifier, setting: Setting, index: i16) -> String {
    if (0..option_count(setting)).contains(&index) {
        i18n::t(
            locale,
            &format!("settings-{}-option-{index}", setting_key(setting)),
        )
    } else {
        index.to_string()
    }
}

/// All option labels of a setting, in stored-index order.
pub(crate) fn option_labels(locale: &LanguageIdentifier, setting: Setting) -> Vec<String> {
    (0..option_count(setting))
        .map(|index| option_label(locale, setting, index))
        .collect()
}

/// Body of the `/settings list` embed: per setting, the current (or
/// default) option label, the description and all options — the legacy
/// settings.js:38-53 layout.
pub(crate) fn render_list_body(
    locale: &LanguageIdentifier,
    effective: &EffectiveSettings,
) -> String {
    ALL_SETTINGS
        .iter()
        .map(|&setting| {
            let options = format!("`{}`", option_labels(locale, setting).join("`, `"));
            [
                i18n::t_args(
                    locale,
                    "settings-list-entry-current",
                    &[
                        ("name", setting_name(locale, setting).into()),
                        (
                            "current",
                            option_label(locale, setting, effective.get(setting)).into(),
                        ),
                    ],
                ),
                i18n::t_args(
                    locale,
                    "settings-list-entry-description",
                    &[("description", setting_description(locale, setting).into())],
                ),
                i18n::t_args(
                    locale,
                    "settings-list-entry-options",
                    &[("options", options.into())],
                ),
            ]
            .join("\n")
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// Confirmation line for a successful `/settings set` (legacy
/// "Setting **{name}** changed to **{label}**.").
pub(crate) fn render_set_confirmation(
    locale: &LanguageIdentifier,
    setting: Setting,
    index: i16,
) -> String {
    i18n::t_args(
        locale,
        "settings-set-success",
        &[
            ("name", setting_name(locale, setting).into()),
            ("value", option_label(locale, setting, index).into()),
        ],
    )
}

/// The `guild_settings` column a setting is stored in.
fn settings_column(setting: Setting) -> guild_settings::Column {
    match setting {
        Setting::DedicationDisplay => guild_settings::Column::DedicationDisplay,
        Setting::StackRoles => guild_settings::Column::StackRoles,
        Setting::HideUnusedTrophies => guild_settings::Column::HideUnusedTrophies,
        Setting::HideQuitUsers => guild_settings::Column::HideQuitUsers,
        Setting::LeaderboardFormat => guild_settings::Column::LeaderboardFormat,
    }
}

/// Writes one setting for a guild: auto-registers the guild row (FK) and
/// upserts the `guild_settings` row, touching only the target column (other
/// settings keep their stored-or-NULL state). Rejects out-of-range indexes
/// defensively (F27) — unreachable through the typed choice enums.
pub(crate) async fn set_setting(
    db: &DatabaseConnection,
    guild_id: i64,
    setting: Setting,
    index: i16,
) -> anyhow::Result<()> {
    anyhow::ensure!(
        (0..option_count(setting)).contains(&index),
        "settings: index {index} out of range for {setting:?}"
    );

    let now = chrono::Utc::now().naive_utc();
    let txn = db.begin().await?;

    guilds::Entity::insert(guilds::ActiveModel {
        id: Set(guild_id),
        is_safe: Set(false),
        created_at: Set(now),
        updated_at: Set(now),
    })
    .on_conflict(OnConflict::column(guilds::Column::Id).do_nothing().to_owned())
    .exec_without_returning(&txn)
    .await?;

    let mut row = guild_settings::ActiveModel {
        guild_id: Set(guild_id),
        created_at: Set(now),
        updated_at: Set(now),
        ..Default::default()
    };
    match setting {
        Setting::DedicationDisplay => row.dedication_display = Set(Some(index)),
        Setting::StackRoles => row.stack_roles = Set(Some(index)),
        Setting::HideUnusedTrophies => row.hide_unused_trophies = Set(Some(index)),
        Setting::HideQuitUsers => row.hide_quit_users = Set(Some(index)),
        Setting::LeaderboardFormat => row.leaderboard_format = Set(Some(index)),
    }

    guild_settings::Entity::insert(row)
        .on_conflict(
            OnConflict::column(guild_settings::Column::GuildId)
                .update_columns([settings_column(setting), guild_settings::Column::UpdatedAt])
                .to_owned(),
        )
        .exec_without_returning(&txn)
        .await?;

    txn.commit().await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Thin handler shared by the five `set` subcommands
// ---------------------------------------------------------------------------

async fn apply_set(ctx: Context<'_>, setting: Setting, chosen: Option<i16>) -> Result<(), Error> {
    let guild_id = require_guild_id(&ctx)?;
    let locale = util::locale(&ctx);
    let index = effective_index(setting, chosen);

    set_setting(&ctx.data().db, guild_id, setting, index).await?;
    log::info!(
        "settings: guild {guild_id} set {setting:?} = {index} (by user {})",
        ctx.author().id
    );

    let embed = serenity::CreateEmbed::new()
        .description(render_set_confirmation(&locale, setting, index))
        .colour(util::COLOR_SUCCESS);
    util::reply_embed(ctx, embed, false).await
}

fn require_guild_id(ctx: &Context<'_>) -> Result<i64, Error> {
    Ok(util::require_guild_id(ctx)?.get() as i64)
}

#[cfg(test)]
mod tests {
    use poise::ChoiceParameter;
    use sea_orm::{ActiveModelTrait, EntityTrait, Set};

    use super::*;
    use crate::domain::settings::get_setting;
    use crate::domain::test_support::{fresh_db, insert_guild, now};
    use crate::i18n;

    fn locale() -> i18n::LanguageIdentifier {
        i18n::resolve(None)
    }

    // -- registration shape (F26) -------------------------------------------

    #[test]
    fn settings_registers_set_group_and_list() {
        let command = settings();
        let mut names: Vec<_> = command.subcommands.iter().map(|c| c.name.to_string()).collect();
        names.sort();
        assert_eq!(names, ["list", "set"]);

        let set = command
            .subcommands
            .iter()
            .find(|c| c.name == "set")
            .expect("set subcommand group");
        let mut setting_names: Vec<_> =
            set.subcommands.iter().map(|c| c.name.to_string()).collect();
        setting_names.sort();
        assert_eq!(
            setting_names,
            [
                "dedication_display",
                "hide_quit_users",
                "hide_unused_trophies",
                "leaderboard_format",
                "stack_roles",
            ]
        );

        // Every (sub)command needs a non-empty description ≤ 100 chars.
        for subcommand in command.subcommands.iter().chain(set.subcommands.iter()) {
            let description = subcommand
                .description
                .as_deref()
                .unwrap_or_else(|| panic!("/settings {} has no description", subcommand.name));
            assert!(!description.is_empty() && description.len() <= 100);
        }
    }

    #[test]
    fn set_subcommands_expose_one_optional_value_with_native_choices() {
        let command = settings();
        let set = command
            .subcommands
            .iter()
            .find(|c| c.name == "set")
            .expect("set subcommand group");

        for (setting, name) in [
            (Setting::DedicationDisplay, "dedication_display"),
            (Setting::StackRoles, "stack_roles"),
            (Setting::HideUnusedTrophies, "hide_unused_trophies"),
            (Setting::HideQuitUsers, "hide_quit_users"),
            (Setting::LeaderboardFormat, "leaderboard_format"),
        ] {
            let subcommand = set
                .subcommands
                .iter()
                .find(|c| c.name == name)
                .unwrap_or_else(|| panic!("missing /settings set {name}"));
            assert_eq!(subcommand.parameters.len(), 1, "{name}: exactly one parameter");
            let value = &subcommand.parameters[0];
            assert_eq!(value.name, "value", "{name}");
            assert!(!value.required, "{name}: value must be optional (reset to default)");
            assert_eq!(
                value.choices.len(),
                option_count(setting) as usize,
                "{name}: one native choice per option"
            );
        }
    }

    // -- choice enums ↔ stored indexes ↔ catalog labels (F26/F27) ------------

    #[test]
    fn choice_lists_match_option_labels_in_index_order() {
        let locale = locale();
        let cases: [(Setting, Vec<poise::CommandParameterChoice>); 5] = [
            (Setting::DedicationDisplay, DedicationDisplayChoice::list()),
            (Setting::StackRoles, StackRolesChoice::list()),
            (Setting::HideUnusedTrophies, HideUnusedTrophiesChoice::list()),
            (Setting::HideQuitUsers, HideQuitUsersChoice::list()),
            (Setting::LeaderboardFormat, LeaderboardFormatChoice::list()),
        ];
        for (setting, choices) in cases {
            assert_eq!(choices.len(), option_count(setting) as usize, "{setting:?}");
            for (index, choice) in choices.iter().enumerate() {
                assert_eq!(
                    choice.name.to_string(),
                    option_label(&locale, setting, index as i16),
                    "{setting:?} option {index}: Discord choice label and catalog label differ"
                );
            }
        }
    }

    #[test]
    fn choice_variants_map_to_documented_indexes() {
        assert_eq!(DedicationDisplayChoice::AlwaysMention as i16, 0);
        assert_eq!(DedicationDisplayChoice::MentionOnlyInServer as i16, 2);
        assert_eq!(StackRolesChoice::OnlyHighestReward as i16, 1);
        assert_eq!(HideUnusedTrophiesChoice::ShowUnusedTrophies as i16, 1);
        assert_eq!(HideQuitUsersChoice::HideQuitUsers as i16, 0);
        assert_eq!(LeaderboardFormatChoice::UsernameAndTag as i16, 3);
    }

    #[test]
    fn omitted_value_resets_to_the_setting_default() {
        for setting in ALL_SETTINGS {
            assert_eq!(effective_index(setting, None), setting.default_value(), "{setting:?}");
        }
        assert_eq!(effective_index(Setting::DedicationDisplay, Some(0)), 0);
    }

    // -- catalog / rendering --------------------------------------------------

    #[test]
    fn catalog_has_name_description_and_every_option_label() {
        let locale = locale();
        for setting in ALL_SETTINGS {
            let name = setting_name(&locale, setting);
            assert!(!name.starts_with("settings-"), "missing name for {setting:?}: {name}");
            let description = setting_description(&locale, setting);
            assert!(
                !description.starts_with("settings-"),
                "missing description for {setting:?}: {description}"
            );
            for index in 0..option_count(setting) {
                let label = option_label(&locale, setting, index);
                assert!(
                    !label.starts_with("settings-"),
                    "missing label for {setting:?} option {index}: {label}"
                );
            }
        }
        for key in ["settings-list-footer", "settings-list-fallback-guild-name"] {
            assert_ne!(i18n::t(&locale, key), key);
        }
        // The title takes an argument, so it must be looked up with one.
        let title = i18n::t_args(&locale, "settings-list-title", &[("guild", "Acme".into())]);
        assert!(title.contains("Acme") && title.contains("Settings"), "got: {title}");
    }

    #[test]
    fn out_of_range_stored_value_renders_as_number_not_crash() {
        let locale = locale();
        assert_eq!(option_label(&locale, Setting::StackRoles, 7), "7");
        assert_eq!(option_label(&locale, Setting::StackRoles, -1), "-1");
    }

    #[test]
    fn list_body_shows_defaults_descriptions_and_options() {
        let locale = locale();
        let defaults = EffectiveSettings {
            dedication_display: 2,
            stack_roles: 1,
            hide_unused_trophies: 1,
            hide_quit_users: 0,
            leaderboard_format: 0,
        };
        let body = render_list_body(&locale, &defaults);

        // One block per setting, legacy order.
        assert_eq!(body.matches("**Options:**").count(), 5, "got: {body}");
        // Current values resolve to the default labels.
        assert!(body.contains("Mention Only in Server"), "got: {body}");
        assert!(body.contains("Only Highest Reward"), "got: {body}");
        // Options are backtick-wrapped and comma-joined like the legacy list.
        assert!(body.contains("`Always Mention`, `Always Name`"), "got: {body}");
        // Descriptions are present.
        assert!(body.contains("hidden from the leaderboard"), "got: {body}");
    }

    #[test]
    fn list_body_reflects_stored_values() {
        let locale = locale();
        let stored = EffectiveSettings {
            dedication_display: 0,
            stack_roles: 0,
            hide_unused_trophies: 0,
            hide_quit_users: 1,
            leaderboard_format: 3,
        };
        let body = strip_isolates(&render_list_body(&locale, &stored));
        assert!(body.contains("**Leaderboard Format:** Username and Tag"), "got: {body}");
        assert!(body.contains("**Stack Roles:** Stack Roles"), "got: {body}");
        assert!(body.contains("**Hide Quit Users:** Show Quit Users"), "got: {body}");
    }

    /// Removes the Unicode directional-isolate marks Fluent wraps
    /// placeables in, so assertions can match plain text.
    fn strip_isolates(text: &str) -> String {
        text.chars().filter(|c| !matches!(c, '\u{2068}' | '\u{2069}')).collect()
    }

    #[test]
    fn set_confirmation_names_setting_and_label() {
        let text = render_set_confirmation(&locale(), Setting::LeaderboardFormat, 2);
        assert!(text.contains("Leaderboard Format"), "got: {text}");
        assert!(text.contains("Nickname"), "got: {text}");
        assert!(text.contains('✅'), "got: {text}");
    }

    // -- DB write path ---------------------------------------------------------

    #[tokio::test]
    async fn set_setting_auto_registers_guild_and_writes_value() {
        let db = fresh_db().await;
        // No guild row yet — set_setting must create it (FK).
        set_setting(&db, 1, Setting::StackRoles, 0).await.expect("set");

        assert!(guilds::Entity::find_by_id(1).one(&db).await.unwrap().is_some());
        // Stored 0 differs from the default (1) and must be respected.
        assert_eq!(get_setting(&db, 1, Setting::StackRoles).await.unwrap(), 0);
    }

    #[tokio::test]
    async fn set_setting_upserts_without_clobbering_other_settings() {
        let db = fresh_db().await;
        insert_guild(&db, 1).await;
        guild_settings::ActiveModel {
            guild_id: Set(1),
            dedication_display: Set(Some(1)),
            stack_roles: Set(None),
            hide_unused_trophies: Set(None),
            hide_quit_users: Set(None),
            leaderboard_format: Set(None),
            created_at: Set(now()),
            updated_at: Set(now()),
        }
        .insert(&db)
        .await
        .expect("insert existing settings row");

        set_setting(&db, 1, Setting::HideQuitUsers, 1).await.expect("set");

        let row = guild_settings::Entity::find_by_id(1)
            .one(&db)
            .await
            .unwrap()
            .expect("single row per guild");
        assert_eq!(row.hide_quit_users, Some(1));
        assert_eq!(row.dedication_display, Some(1), "other stored column untouched");
        assert_eq!(row.stack_roles, None, "unset columns stay NULL (default)");
    }

    #[tokio::test]
    async fn set_setting_overwrites_the_same_setting() {
        let db = fresh_db().await;
        set_setting(&db, 1, Setting::LeaderboardFormat, 3).await.expect("first set");
        set_setting(&db, 1, Setting::LeaderboardFormat, 1).await.expect("second set");
        assert_eq!(get_setting(&db, 1, Setting::LeaderboardFormat).await.unwrap(), 1);
    }

    #[tokio::test]
    async fn set_setting_is_scoped_to_its_guild() {
        let db = fresh_db().await;
        set_setting(&db, 1, Setting::HideQuitUsers, 1).await.expect("set guild 1");
        assert_eq!(
            get_setting(&db, 2, Setting::HideQuitUsers).await.unwrap(),
            Setting::HideQuitUsers.default_value()
        );
    }

    #[tokio::test]
    async fn set_setting_rejects_out_of_range_indexes() {
        let db = fresh_db().await;
        assert!(set_setting(&db, 1, Setting::StackRoles, 2).await.is_err());
        assert!(set_setting(&db, 1, Setting::StackRoles, -1).await.is_err());
        // Nothing was written.
        assert!(guild_settings::Entity::find_by_id(1).one(&db).await.unwrap().is_none());
    }
}
