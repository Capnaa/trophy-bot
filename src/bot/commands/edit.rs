//! `/edit` — edit an existing trophy (batch C8).
//!
//! Spec: docs/specs/commands-trophy-management.md §/edit. Parity fixes:
//! - F6: the image goes through the shared pipeline (`src/bot/images.rs`) —
//!   content-type + 1 MB validated from attachment metadata BEFORE download,
//!   the file is stored locally and the DB keeps the **filename**, never the
//!   expiring Discord CDN URL (legacy stored the URL and its dead size check
//!   never ran).
//! - F7: `value` and `signed` are intentionally NOT editable — there are no
//!   options for them and [`apply_edit`] never touches those columns (nor
//!   `creator_user_id`/`created_at`). Kept immutable at cutover for parity
//!   with the legacy bot; a candidate for a later release.
//! - F37: accurate change report — editing a field (incl. the dedication) to
//!   its current value is NOT a change, and clearing the dedication with the
//!   legacy `"-"` sentinel is reported cleanly as "(none)", not "null".
//! - F12 (shared): the trophy is resolved by exact normalized name with
//!   autocomplete via `src/bot/resolver.rs`.
//!
//! Renaming re-checks the per-guild normalized-name uniqueness (excluding the
//! trophy itself, so cosmetic renames like "gold medal" → "Gold Medal" are
//! allowed) and keeps `normalized_name` in sync.
//!
//! Business logic lives in plain testable functions ([`plan_edit`],
//! [`apply_edit`], [`rename_collides`], [`render_changes`]); the poise
//! handler at the bottom stays thin. Validation limits and their localized
//! messages are shared with `/create` (same rules, same wording).

use poise::serenity_prelude as serenity;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DatabaseConnection, EntityTrait,
    PaginatorTrait, QueryFilter, Set,
};
use uuid::Uuid;

use crate::bot::commands::create::{
    dedication_columns, parse_dedication, validate_fields, CreateError, Dedication,
};
use crate::bot::{images, resolver, util, Context, Error};
use crate::domain::normalize::normalize_name;
use crate::entities::trophies;
use crate::i18n::{self, LanguageIdentifier};

/// Legacy sentinel: a dedication of `"-"` clears the dedication.
const CLEAR_DEDICATION: &str = "-";

/// The caller-provided edits, already parsed/resolved (no Discord types so
/// the planning logic stays purely testable).
#[derive(Debug, Clone, Default)]
pub(crate) struct EditRequest {
    pub name: Option<String>,
    pub description: Option<String>,
    pub emoji: Option<String>,
    /// `None` = option not provided; `Some((None, None))` = cleared via `"-"`;
    /// otherwise the resolved `(dedication_user_id, dedication_text)` pair.
    pub dedication: Option<(Option<i64>, Option<String>)>,
    pub details: Option<String>,
    /// New stored image filename when an attachment was provided.
    pub image: Option<String>,
    /// `None` = option not provided; `Some(None)` = cleared (blank input);
    /// `Some(Some(x))` = new category.
    pub category: Option<Option<String>>,
    pub active: Option<bool>,
}

/// One entry of the change report (F37: built only from REAL changes).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Change {
    Name { old: String, new: String },
    Description { old: String, new: String },
    Emoji { old: String, new: String },
    /// Display-ready sides; `None` renders as the localized "(none)".
    Dedication { old: Option<String>, new: Option<String> },
    Details { old: String, new: String },
    Image,
    Category { old: Option<String>, new: Option<String> },
    Active { old: bool, new: bool },
}

/// The merged post-edit field values plus the change report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EditPlan {
    pub name: String,
    pub normalized_name: String,
    pub description: String,
    pub emoji: String,
    pub dedication_user_id: Option<i64>,
    pub dedication_text: Option<String>,
    pub details: String,
    pub image: Option<String>,
    pub category: Option<String>,
    pub active: bool,
    pub changes: Vec<Change>,
}

/// Display form of a stored dedication: mention for a user dedication, the
/// text otherwise, `None` when there is no dedication.
fn dedication_display(user_id: Option<i64>, text: Option<&str>) -> Option<String> {
    match (user_id, text) {
        (Some(id), _) => Some(format!("<@{id}>")),
        (None, Some(t)) => Some(t.to_string()),
        (None, None) => None,
    }
}

/// Merges the request over the current trophy and builds the change report.
/// A provided value identical to the stored one is NOT a change (F37); a
/// provided attachment always is (same filename, new bytes).
pub(crate) fn plan_edit(current: &trophies::Model, request: EditRequest) -> EditPlan {
    let mut changes = Vec::new();

    let name = match request.name {
        Some(new) if new != current.name => {
            changes.push(Change::Name { old: current.name.clone(), new: new.clone() });
            new
        }
        _ => current.name.clone(),
    };
    let description = match request.description {
        Some(new) if new != current.description => {
            changes
                .push(Change::Description { old: current.description.clone(), new: new.clone() });
            new
        }
        _ => current.description.clone(),
    };
    let emoji = match request.emoji {
        Some(new) if new != current.emoji => {
            changes.push(Change::Emoji { old: current.emoji.clone(), new: new.clone() });
            new
        }
        _ => current.emoji.clone(),
    };

    // Dedication: same target user (regardless of the stored display-name
    // snapshot) or same text = no change (F37).
    let (dedication_user_id, dedication_text) = match request.dedication {
        Some((new_user, new_text)) => {
            let same = match (new_user, current.dedication_user_id) {
                (Some(a), Some(b)) => a == b,
                (None, None) => new_text == current.dedication_text,
                _ => false,
            };
            if same {
                (current.dedication_user_id, current.dedication_text.clone())
            } else {
                changes.push(Change::Dedication {
                    old: dedication_display(
                        current.dedication_user_id,
                        current.dedication_text.as_deref(),
                    ),
                    new: dedication_display(new_user, new_text.as_deref()),
                });
                (new_user, new_text)
            }
        }
        None => (current.dedication_user_id, current.dedication_text.clone()),
    };

    let details = match request.details {
        Some(new) if new != current.details => {
            changes.push(Change::Details { old: current.details.clone(), new: new.clone() });
            new
        }
        _ => current.details.clone(),
    };

    let image = match request.image {
        Some(new) => {
            changes.push(Change::Image);
            Some(new)
        }
        None => current.image.clone(),
    };

    let category = match request.category {
        Some(new) if new != current.category => {
            changes.push(Change::Category { old: current.category.clone(), new: new.clone() });
            new
        }
        _ => current.category.clone(),
    };

    let active = match request.active {
        Some(new) if new != current.active => {
            changes.push(Change::Active { old: current.active, new });
            new
        }
        _ => current.active,
    };

    EditPlan {
        normalized_name: normalize_name(&name),
        name,
        description,
        emoji,
        dedication_user_id,
        dedication_text,
        details,
        image,
        category,
        active,
        changes,
    }
}

/// Renders the change report as one localized line per change.
pub(crate) fn render_changes(locale: &LanguageIdentifier, changes: &[Change]) -> String {
    let none = i18n::t(locale, "edit-dedication-none");
    changes
        .iter()
        .map(|change| match change {
            Change::Name { old, new } => line(locale, "edit-change-name", old, new),
            Change::Description { old, new } => line(locale, "edit-change-description", old, new),
            Change::Emoji { old, new } => line(locale, "edit-change-emoji", old, new),
            Change::Details { old, new } => line(locale, "edit-change-details", old, new),
            Change::Dedication { old, new } => line(
                locale,
                "edit-change-dedication",
                old.as_deref().unwrap_or(&none),
                new.as_deref().unwrap_or(&none),
            ),
            Change::Image => i18n::t(locale, "edit-change-image"),
            Change::Category { old, new } => line(
                locale,
                "edit-change-category",
                old.as_deref().unwrap_or(&none),
                new.as_deref().unwrap_or(&none),
            ),
            Change::Active { old, new } => {
                line(locale, "edit-change-active", &active_label(locale, *old), &active_label(locale, *new))
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn active_label(locale: &LanguageIdentifier, active: bool) -> String {
    i18n::t(locale, if active { "edit-active-yes" } else { "edit-active-no" })
}

fn line(locale: &LanguageIdentifier, key: &str, old: &str, new: &str) -> String {
    i18n::t_args(locale, key, &[("old", old.to_string().into()), ("new", new.to_string().into())])
}

/// True when renaming to `normalized_name` would collide with ANOTHER trophy
/// of the guild (the trophy itself is excluded so cosmetic renames pass).
pub(crate) async fn rename_collides(
    db: &impl ConnectionTrait,
    guild_id: i64,
    self_id: Uuid,
    normalized_name: &str,
) -> anyhow::Result<bool> {
    let duplicates = trophies::Entity::find()
        .filter(trophies::Column::GuildId.eq(guild_id))
        .filter(trophies::Column::NormalizedName.eq(normalized_name))
        .filter(trophies::Column::Id.ne(self_id))
        .count(db)
        .await?;
    Ok(duplicates > 0)
}

/// Persists the plan. `value`, `signed`, `creator_user_id` and `created_at`
/// are never written (F7). The inner `Err` is the race-window rename
/// collision (unique index on `(guild_id, normalized_name)` fired between
/// [`rename_collides`] and the update).
pub(crate) async fn apply_edit(
    db: &DatabaseConnection,
    current: trophies::Model,
    plan: &EditPlan,
) -> anyhow::Result<Result<trophies::Model, CreateError>> {
    let mut active: trophies::ActiveModel = current.into();
    active.name = Set(plan.name.clone());
    active.normalized_name = Set(plan.normalized_name.clone());
    active.description = Set(plan.description.clone());
    active.emoji = Set(plan.emoji.clone());
    active.dedication_user_id = Set(plan.dedication_user_id);
    active.dedication_text = Set(plan.dedication_text.clone());
    active.details = Set(plan.details.clone());
    active.image = Set(plan.image.clone());
    active.category = Set(plan.category.clone());
    active.active = Set(plan.active);
    active.updated_at = Set(chrono::Utc::now().naive_utc());

    match active.update(db).await {
        Ok(model) => Ok(Ok(model)),
        Err(err)
            if matches!(err.sql_err(), Some(sea_orm::SqlErr::UniqueConstraintViolation(_))) =>
        {
            Ok(Err(CreateError::DuplicateName { name: plan.name.clone() }))
        }
        Err(err) => Err(err.into()),
    }
}

/// Resolves the raw dedication option: the legacy `"-"` sentinel clears it;
/// a mention/snowflake becomes a user dedication with a display-name
/// snapshot; an UNRESOLVABLE mention/snowflake falls back to a text
/// dedication with the raw input; anything else is plain text — all exactly
/// the `/create` semantics (see `create::dedication_columns`).
async fn resolve_dedication(ctx: &Context<'_>, raw: &str) -> (Option<i64>, Option<String>) {
    if raw.trim() == CLEAR_DEDICATION {
        return (None, None);
    }
    let parsed = parse_dedication(raw);
    let fetched = match parsed {
        Dedication::User(id) => {
            match serenity::UserId::new(id).to_user(ctx.serenity_context()).await {
                Ok(user) => Some(user.name.clone()),
                Err(err) => {
                    log::debug!(
                        "could not resolve dedication user {id}, storing the raw text instead: {err}"
                    );
                    None
                }
            }
        }
        Dedication::Text(_) => None,
    };
    dedication_columns(raw, parsed, fetched)
}

/// Temp name a replacement image is downloaded under before the DB update.
/// Derived from the final `{guild_id}_{trophy_uuid}.{ext}` name, so it never
/// collides with any stored filename (stored names never end in `.tmp`).
pub(crate) fn temp_filename(filename: &str) -> String {
    format!("{filename}.tmp")
}

/// Promotes a downloaded temp image over its final filename once the DB
/// update went through. Best-effort: past the commit an error can only be
/// logged — the trophy then keeps serving the previous same-named file (or
/// none), which beats having clobbered it before the update.
pub(crate) async fn promote_image(temp: &str, dest: &str) {
    promote_image_in(std::path::Path::new(images::IMAGES_DIR), temp, dest).await;
}

/// Directory-parameterized body of [`promote_image`] so tests run against a
/// throwaway directory instead of the real `images/`.
async fn promote_image_in(dir: &std::path::Path, temp: &str, dest: &str) {
    if let Err(err) = tokio::fs::rename(dir.join(temp), dir.join(dest)).await {
        log::error!("failed to promote trophy image {temp} -> {dest}: {err}");
        // Same traversal guard as images::remove, scoped to `dir`.
        if images::is_plain_filename(temp)
            && let Err(err) = tokio::fs::remove_file(dir.join(temp)).await
            && err.kind() != std::io::ErrorKind::NotFound
        {
            log::warn!("failed to drop temp image {temp}: {err}");
        }
    }
}

/// Edit an existing trophy for your server.
#[allow(clippy::too_many_arguments)]
#[poise::command(slash_command, guild_only, default_member_permissions = "MANAGE_GUILD", required_permissions = "MANAGE_GUILD")]
pub async fn edit(
    ctx: Context<'_>,
    #[description = "The trophy to be edited"]
    #[autocomplete = "resolver::autocomplete_trophy"]
    trophy: String,
    #[description = "The new name of the trophy."]
    #[min_length = 1]
    #[max_length = 32]
    name: Option<String>,
    #[description = "The new description for the trophy"] description: Option<String>,
    #[description = "A new emoji for the trophy"] emoji: Option<String>,
    #[description = "A new dedication for the trophy. Use - to remove the current one"]
    dedication: Option<String>,
    #[description = "A new details text for the trophy"] details: Option<String>,
    #[description = "A new image for the trophy"] image: Option<serenity::Attachment>,
    #[description = "A new category for the trophy. Use an empty string to clear it"]
    category: Option<String>,
    #[description = "Whether this medal can currently be awarded"] active: Option<bool>,
) -> Result<(), Error> {
    let locale = util::locale(&ctx);
    // Effective guild (guild_links): a linked guild's /edit operates on the
    // SOURCE guild's trophy it mirrors, same as /create.
    let guild_id = util::effective_guild_id(&ctx).await?.get() as i64;
    let db = &ctx.data().db;

    let Some(current) =
        resolver::resolve_trophy_or_reply(ctx, guild_id, &trophy, "edit-error-not-found").await?
    else {
        return Ok(());
    };

    // Validate BEFORE any download or write — shared rules (and localized
    // messages) with /create; absent options fall back to the stored values,
    // which are already within limits. `value` is not editable (F7), so the
    // stored value is passed through.
    if let Err(err) = validate_fields(
        name.as_deref().unwrap_or(&current.name),
        description.as_deref(),
        emoji.as_deref(),
        current.value,
        dedication.as_deref(),
        details.as_deref(),
    ) {
        return util::reply_error(ctx, err.message(&locale), true).await;
    }
    if let Err(err) = crate::bot::commands::create::validate_category(category.as_deref()) {
        return util::reply_error(ctx, err.message(&locale), true).await;
    }

    // Image metadata check (F6) before anything is downloaded.
    let image_plan = match &image {
        None => None,
        Some(attachment) => {
            match images::validate(attachment.content_type.as_deref(), attachment.size) {
                Ok(ext) => Some((attachment.url.clone(), ext)),
                Err(images::ImageError::UnsupportedType) => {
                    return util::reply_error(
                        ctx,
                        i18n::t(&locale, "create-error-image-type"),
                        true,
                    )
                    .await;
                }
                Err(images::ImageError::TooLarge) => {
                    return util::reply_error(
                        ctx,
                        i18n::t(&locale, "create-error-image-too-large"),
                        true,
                    )
                    .await;
                }
            }
        }
    };

    // Renaming must re-check per-guild uniqueness, excluding this trophy.
    let new_normalized = name.as_deref().map(normalize_name);
    if let Some(normalized) = &new_normalized
        && *normalized != current.normalized_name
        && rename_collides(db, guild_id, current.id, normalized).await?
    {
        let err = CreateError::DuplicateName { name: name.clone().unwrap_or_default() };
        return util::reply_error(ctx, err.message(&locale), true).await;
    }

    // Defer before the first slow await: resolve_dedication does a live
    // to_user HTTP fetch (for a mention/ID dedication) and the image download
    // follows — either can exceed Discord's ~3s window. Deferring only for the
    // image left the no-image + cache-miss-dedication path timing out while
    // still committing the edit. The defer is PUBLIC (success is public), so
    // every error reply past this point uses `reply_error_ephemeral` (§2).
    ctx.defer().await?;

    let dedication_pair = match dedication.as_deref() {
        None => None,
        Some(raw) => Some(resolve_dedication(&ctx, raw).await),
    };

    // Empty/whitespace-only input clears the category; anything else sets it.
    let category = category.map(|raw| (!raw.trim().is_empty()).then_some(raw));

    let request = EditRequest {
        name,
        description,
        emoji,
        dedication: dedication_pair,
        details,
        image: image_plan
            .as_ref()
            .map(|(_, ext)| images::filename(guild_id, current.id, ext)),
        category,
        active,
    };
    let plan = plan_edit(&current, request);

    if plan.changes.is_empty() {
        return util::reply_error_ephemeral(ctx, i18n::t(&locale, "edit-error-no-changes")).await;
    }

    // Download (slow path) only after every check passed. The DB stores the
    // local filename, never the CDN URL (F6). The bytes land under a temp name
    // and are only promoted over the final filename AFTER the DB update
    // succeeds: a same-extension replacement reuses the stored filename, and
    // writing it in place before the update would irrecoverably clobber the
    // old image if the update then failed.
    let old_image = current.image.clone();
    let old_category = current.category.clone();
    let new_image = plan.image.clone().filter(|_| image_plan.is_some());
    let temp_image = new_image.as_deref().map(temp_filename);
    if let (Some((url, _)), Some(temp)) = (&image_plan, &temp_image)
        && let Err(err) = images::download(url, temp).await
    {
        log::warn!("/edit image download failed (guild={guild_id}): {err:#}");
        return util::reply_error_ephemeral(ctx, i18n::t(&locale, "create-error-image-download"))
            .await;
    }

    let updated = match apply_edit(db, current, &plan).await {
        Ok(Ok(model)) => model,
        Ok(Err(err)) => {
            // The temp file never shadows a stored filename: dropping it
            // leaves the previous image fully intact.
            if let Some(temp) = &temp_image {
                images::remove(temp).await;
            }
            // Race-window rename collision: may run after the public defer.
            return util::reply_error_ephemeral(ctx, err.message(&locale)).await;
        }
        Err(err) => {
            if let Some(temp) = &temp_image {
                images::remove(temp).await;
            }
            return Err(err);
        }
    };

    if let (Some(temp), Some(new)) = (&temp_image, &new_image) {
        promote_image(temp, new).await;
    }

    // Refresh whichever category catalog panel(s) this edit could affect:
    // the trophy's (possibly unchanged) current category — covers it
    // gaining/losing active status and any name/emoji/description change —
    // plus its OLD category too when the category itself changed (a medal
    // moving out of one category's panel and into another's).
    if let Some(category) = &updated.category {
        ctx.data().medals_panel_signal.notify(guild_id, category.clone());
    }
    if old_category != updated.category
        && let Some(category) = &old_category
    {
        ctx.data().medals_panel_signal.notify(guild_id, category.clone());
    }

    // The previous image file is orphaned once replaced by one with a
    // different name (extension changed or legacy-named file).
    if let (Some(new), Some(old)) = (&new_image, &old_image)
        && new != old
    {
        images::remove(old).await;
    }

    let embed = serenity::CreateEmbed::new()
        .title(i18n::t(&locale, "edit-success-title"))
        .description(render_changes(&locale, &plan.changes))
        .colour(util::COLOR_SUCCESS)
        .footer(serenity::CreateEmbedFooter::new(i18n::t_args(
            &locale,
            "edit-footer",
            &[("name", updated.name.into())],
        )));
    util::reply_embed(ctx, embed, false).await
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::domain::test_support::{fresh_db, insert_guild, now};
    use crate::entities::guilds;

    /// A fully populated trophy model to plan edits against.
    fn model(guild_id: i64, name: &str) -> trophies::Model {
        trophies::Model {
            id: Uuid::now_v7(),
            guild_id,
            legacy_id: None,
            creator_user_id: Some(7),
            name: name.to_string(),
            normalized_name: normalize_name(name),
            description: "Old description".to_string(),
            emoji: "🏆".to_string(),
            value: 10,
            image: None,
            dedication_user_id: None,
            dedication_text: None,
            details: "Old details".to_string(),
            signed: false,
            category: None,
            active: true,
            created_at: now(),
            updated_at: now(),
        }
    }

    async fn insert(db: &DatabaseConnection, m: trophies::Model) -> trophies::Model {
        if guilds::Entity::find_by_id(m.guild_id).one(db).await.unwrap().is_none() {
            insert_guild(db, m.guild_id).await;
        }
        let active: trophies::ActiveModel = m.into();
        trophies::Entity::insert(active).exec_with_returning(db).await.expect("insert trophy")
    }

    fn locale() -> LanguageIdentifier {
        i18n::resolve(None)
    }

    // --- plan_edit: change detection (F37) ---

    #[test]
    fn empty_request_plans_no_changes() {
        let current = model(1, "Gold Medal");
        let plan = plan_edit(&current, EditRequest::default());
        assert!(plan.changes.is_empty());
        assert_eq!(plan.name, "Gold Medal");
        assert_eq!(plan.normalized_name, "goldmedal");
        assert_eq!(plan.description, "Old description");
        assert_eq!(plan.image, None);
    }

    #[test]
    fn same_values_are_not_changes() {
        // F37 generalized: providing a field's current value is a no-op.
        let mut current = model(1, "Gold Medal");
        current.dedication_text = Some("mom".to_string());
        let plan = plan_edit(
            &current,
            EditRequest {
                name: Some("Gold Medal".into()),
                description: Some("Old description".into()),
                emoji: Some("🏆".into()),
                dedication: Some((None, Some("mom".into()))),
                details: Some("Old details".into()),
                image: None,
                category: None,
                active: None,
            },
        );
        assert!(plan.changes.is_empty(), "changes: {:?}", plan.changes);
    }

    #[test]
    fn changed_fields_are_merged_and_reported() {
        let current = model(1, "Gold Medal");
        let plan = plan_edit(
            &current,
            EditRequest {
                name: Some("Platinum Medal".into()),
                description: Some("New description".into()),
                emoji: Some("🥇".into()),
                details: Some("New details".into()),
                ..EditRequest::default()
            },
        );
        assert_eq!(plan.name, "Platinum Medal");
        assert_eq!(plan.normalized_name, "platinummedal", "normalized_name follows the rename");
        assert_eq!(plan.description, "New description");
        assert_eq!(plan.emoji, "🥇");
        assert_eq!(plan.details, "New details");
        assert_eq!(
            plan.changes,
            vec![
                Change::Name { old: "Gold Medal".into(), new: "Platinum Medal".into() },
                Change::Description {
                    old: "Old description".into(),
                    new: "New description".into()
                },
                Change::Emoji { old: "🏆".into(), new: "🥇".into() },
                Change::Details { old: "Old details".into(), new: "New details".into() },
            ]
        );
    }

    #[test]
    fn dedication_same_user_is_not_a_change_even_with_new_name_snapshot() {
        // The stored display-name snapshot may drift; same user = same value.
        let mut current = model(1, "Gold Medal");
        current.dedication_user_id = Some(42);
        current.dedication_text = Some("old-username".to_string());
        let plan = plan_edit(
            &current,
            EditRequest {
                dedication: Some((Some(42), Some("new-username".into()))),
                ..EditRequest::default()
            },
        );
        assert!(plan.changes.is_empty());
        assert_eq!(plan.dedication_text.as_deref(), Some("old-username"), "snapshot kept");
    }

    #[test]
    fn clearing_an_absent_dedication_is_not_a_change() {
        // F37: `"-"` on a trophy with no dedication reports nothing.
        let current = model(1, "Gold Medal");
        let plan =
            plan_edit(&current, EditRequest { dedication: Some((None, None)), ..Default::default() });
        assert!(plan.changes.is_empty());
    }

    #[test]
    fn dedication_transitions_are_changes() {
        // text -> user
        let mut current = model(1, "Gold Medal");
        current.dedication_text = Some("mom".to_string());
        let plan = plan_edit(
            &current,
            EditRequest {
                dedication: Some((Some(42), Some("ana".into()))),
                ..EditRequest::default()
            },
        );
        assert_eq!(plan.dedication_user_id, Some(42));
        assert_eq!(
            plan.changes,
            vec![Change::Dedication { old: Some("mom".into()), new: Some("<@42>".into()) }]
        );

        // user -> cleared with "-"
        let mut current = model(1, "Gold Medal");
        current.dedication_user_id = Some(42);
        current.dedication_text = Some("ana".to_string());
        let plan =
            plan_edit(&current, EditRequest { dedication: Some((None, None)), ..Default::default() });
        assert_eq!(plan.dedication_user_id, None);
        assert_eq!(plan.dedication_text, None);
        assert_eq!(
            plan.changes,
            vec![Change::Dedication { old: Some("<@42>".into()), new: None }]
        );
    }

    #[test]
    fn new_image_is_always_a_change_and_replaces_the_filename() {
        let mut current = model(1, "Gold Medal");
        current.image = Some("1_old.png".to_string());
        let plan = plan_edit(
            &current,
            EditRequest { image: Some("1_new.gif".into()), ..EditRequest::default() },
        );
        assert_eq!(plan.image.as_deref(), Some("1_new.gif"));
        assert_eq!(plan.changes, vec![Change::Image]);

        // No attachment keeps the stored filename.
        let plan = plan_edit(&current, EditRequest::default());
        assert_eq!(plan.image.as_deref(), Some("1_old.png"));
    }

    #[test]
    fn category_set_clear_and_no_op() {
        let current = model(1, "Gold Medal"); // category: None

        // Setting a category is a change.
        let plan = plan_edit(
            &current,
            EditRequest { category: Some(Some("Government".into())), ..Default::default() },
        );
        assert_eq!(plan.category.as_deref(), Some("Government"));
        assert_eq!(plan.changes, vec![Change::Category { old: None, new: Some("Government".into()) }]);

        // Clearing an already-absent category is not a change.
        let plan =
            plan_edit(&current, EditRequest { category: Some(None), ..Default::default() });
        assert!(plan.changes.is_empty());

        // Setting the same category again is not a change.
        let mut categorized = current.clone();
        categorized.category = Some("Government".to_string());
        let plan = plan_edit(
            &categorized,
            EditRequest { category: Some(Some("Government".into())), ..Default::default() },
        );
        assert!(plan.changes.is_empty());

        // Clearing a set category is a change.
        let plan =
            plan_edit(&categorized, EditRequest { category: Some(None), ..Default::default() });
        assert_eq!(plan.category, None);
        assert_eq!(
            plan.changes,
            vec![Change::Category { old: Some("Government".into()), new: None }]
        );
    }

    #[test]
    fn active_toggle_and_no_op() {
        let current = model(1, "Gold Medal"); // active: true

        let plan = plan_edit(&current, EditRequest { active: Some(false), ..Default::default() });
        assert!(!plan.active);
        assert_eq!(plan.changes, vec![Change::Active { old: true, new: false }]);

        // Same value is not a change.
        let plan = plan_edit(&current, EditRequest { active: Some(true), ..Default::default() });
        assert!(plan.changes.is_empty());
    }

    // --- render_changes (F37 formatting) ---

    #[test]
    fn rendered_report_shows_old_and_new_values() {
        let text = render_changes(
            &locale(),
            &[
                Change::Name { old: "Gold".into(), new: "Platinum".into() },
                Change::Image,
            ],
        );
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("Gold") && lines[0].contains("Platinum"), "got: {}", lines[0]);
    }

    #[test]
    fn cleared_dedication_renders_none_not_null() {
        let text = render_changes(
            &locale(),
            &[Change::Dedication { old: Some("<@42>".into()), new: None }],
        );
        assert!(text.contains("<@42>"), "got: {text}");
        assert!(text.contains("(none)"), "got: {text}");
        assert!(!text.contains("null"), "got: {text}");
    }

    // --- rename_collides / apply_edit (DB) ---

    #[tokio::test]
    async fn rename_collision_excludes_the_trophy_itself() {
        let db = fresh_db().await;
        let gold = insert(&db, model(1, "Gold Medal")).await;
        insert(&db, model(1, "Silver Medal")).await;
        insert(&db, model(2, "Bronze Medal")).await;

        // Cosmetic rename (same normalized name) does not collide.
        assert!(!rename_collides(&db, 1, gold.id, "goldmedal").await.unwrap());
        // Another guild's trophy does not collide.
        assert!(!rename_collides(&db, 1, gold.id, "bronzemedal").await.unwrap());
        // A sibling trophy does.
        assert!(rename_collides(&db, 1, gold.id, "silvermedal").await.unwrap());
    }

    #[tokio::test]
    async fn apply_edit_persists_the_plan_and_keeps_value_signed_creator() {
        let db = fresh_db().await;
        let mut initial = model(1, "Gold Medal");
        initial.value = -5;
        initial.signed = true;
        let current = insert(&db, initial).await;
        let created_at = current.created_at;

        let plan = plan_edit(
            &current,
            EditRequest {
                name: Some("Platinum Medal".into()),
                description: Some("Shinier".into()),
                dedication: Some((Some(42), Some("ana".into()))),
                image: Some("1_x.gif".into()),
                ..EditRequest::default()
            },
        );
        let updated = apply_edit(&db, current, &plan).await.unwrap().unwrap();

        assert_eq!(updated.name, "Platinum Medal");
        assert_eq!(updated.normalized_name, "platinummedal");
        assert_eq!(updated.description, "Shinier");
        assert_eq!(updated.dedication_user_id, Some(42));
        assert_eq!(updated.image.as_deref(), Some("1_x.gif"));
        // F7: immutable fields survive untouched.
        assert_eq!(updated.value, -5);
        assert!(updated.signed);
        assert_eq!(updated.creator_user_id, Some(7));
        assert_eq!(updated.created_at, created_at);

        // The rename is queryable: new name resolves, old does not.
        let by_new = resolver::resolve_trophy(&db, 1, "platinum-MEDAL").await.unwrap();
        assert_eq!(by_new.map(|t| t.id), Some(updated.id));
        assert!(resolver::resolve_trophy(&db, 1, "Gold Medal").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn apply_edit_race_collision_maps_to_duplicate_name() {
        let db = fresh_db().await;
        insert(&db, model(1, "Silver Medal")).await;
        let gold = insert(&db, model(1, "Gold Medal")).await;

        // Rename straight into the sibling's normalized name (the unique
        // index is the last line of defense past the precheck).
        let plan = plan_edit(
            &gold,
            EditRequest { name: Some("SILVER! medal".into()), ..EditRequest::default() },
        );
        let result = apply_edit(&db, gold.clone(), &plan).await.expect("infra ok");
        assert_eq!(result, Err(CreateError::DuplicateName { name: "SILVER! medal".into() }));

        // Nothing was written.
        let still = trophies::Entity::find_by_id(gold.id).one(&db).await.unwrap().unwrap();
        assert_eq!(still.name, "Gold Medal");
    }

    #[tokio::test]
    async fn cosmetic_rename_to_same_normalized_name_succeeds() {
        let db = fresh_db().await;
        let current = insert(&db, model(1, "gold medal")).await;

        let plan = plan_edit(
            &current,
            EditRequest { name: Some("Gold Medal".into()), ..EditRequest::default() },
        );
        assert_eq!(
            plan.changes,
            vec![Change::Name { old: "gold medal".into(), new: "Gold Medal".into() }]
        );
        let updated = apply_edit(&db, current, &plan).await.unwrap().unwrap();
        assert_eq!(updated.name, "Gold Medal");
        assert_eq!(updated.normalized_name, "goldmedal");
    }

    // --- temp image promotion ---

    #[test]
    fn temp_filename_appends_tmp_and_never_matches_a_stored_name() {
        let stored = images::filename(1, Uuid::now_v7(), "png");
        let temp = temp_filename(&stored);
        assert_eq!(temp, format!("{stored}.tmp"));
        assert_ne!(temp, stored);
    }

    /// Unique throwaway dir under the OS temp dir — tests must never write
    /// into the real `images/` (leftovers there would count as orphan disk
    /// files in the import report).
    fn scratch_dir() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("trophy-bot-test-{}", Uuid::now_v7()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[tokio::test]
    async fn promote_image_replaces_the_final_file_only_on_promotion() {
        let dir = scratch_dir();
        let dest = images::filename(1, Uuid::now_v7(), "png");
        let temp = temp_filename(&dest);
        std::fs::write(dir.join(&dest), b"old bytes").unwrap();
        std::fs::write(dir.join(&temp), b"new bytes").unwrap();

        // Until promotion the final file still holds the old bytes (the DB
        // failure path only drops the temp and never touches the original).
        assert_eq!(std::fs::read(dir.join(&dest)).unwrap(), b"old bytes");

        promote_image_in(&dir, &temp, &dest).await;
        assert_eq!(std::fs::read(dir.join(&dest)).unwrap(), b"new bytes");
        assert!(!dir.join(&temp).exists(), "temp file is consumed");

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test]
    async fn promote_image_with_missing_temp_leaves_the_final_file_alone() {
        let dir = scratch_dir();
        let dest = images::filename(1, Uuid::now_v7(), "png");
        std::fs::write(dir.join(&dest), b"old bytes").unwrap();

        promote_image_in(&dir, &temp_filename(&dest), &dest).await;
        assert_eq!(std::fs::read(dir.join(&dest)).unwrap(), b"old bytes");

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn handler_defers_before_slow_awaits() {
        // resolve_dedication does a live to_user HTTP fetch for a mention/ID
        // dedication; ctx.defer() MUST precede it (and the image download)
        // or a cache-miss dedication times the interaction out.
        let src = include_str!("edit.rs");
        let handler = src.split("pub async fn edit").nth(1).expect("handler exists");
        let defer = handler.find("ctx.defer()").expect("handler defers");
        let dedication = handler.find("resolve_dedication(").expect("handler resolves dedication");
        assert!(defer < dedication, "ctx.defer() must come before the to_user HTTP fetch");
    }

    /// Same guard as in `create.rs`: after the PUBLIC defer, error replies
    /// must use `reply_error_ephemeral` (§2).
    #[test]
    fn errors_after_the_public_defer_are_ephemeral() {
        let src = include_str!("edit.rs");
        let handler = src.split("pub async fn edit").nth(1).expect("handler exists");
        let defer = handler.find("ctx.defer()").expect("image path defers");
        let plain_reply_error = concat!("util::reply_", "error(");
        assert!(
            !handler[defer..].contains(plain_reply_error),
            "an error path after ctx.defer() uses the plain reply_error; \
             use util::reply_error_ephemeral instead"
        );
    }

    // --- i18n catalog ---

    #[test]
    fn all_edit_messages_exist() {
        let locale = locale();
        let args: &[(&'static str, i18n::FluentValue<'static>)] = &[
            ("input", "Gold".into()),
            ("name", "Gold".into()),
            ("old", "a".into()),
            ("new", "b".into()),
        ];
        for key in [
            "edit-error-not-found",
            "edit-error-no-changes",
            "edit-success-title",
            "edit-footer",
            "edit-change-name",
            "edit-change-description",
            "edit-change-emoji",
            "edit-change-dedication",
            "edit-change-details",
            "edit-change-image",
            "edit-dedication-none",
            "edit-change-category",
            "edit-change-active",
            "edit-active-yes",
            "edit-active-no",
        ] {
            assert_ne!(i18n::t_args(&locale, key, args), key, "missing ftl message: {key}");
        }
    }
}
