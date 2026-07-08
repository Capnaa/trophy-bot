//! `/details` — show a trophy's private details text (batch C10).
//!
//! Spec: docs/specs/commands-trophy-management.md §/details. Fixes applied:
//! - F11: the reply is EPHEMERAL (legacy posted the "private" details in a
//!   public message) and Manage Guild is enforced like the rest of the
//!   management set (`default_member_permissions` below).
//! - F12: trophy resolved by exact normalized name with autocomplete via the
//!   shared resolver (`src/bot/resolver.rs`) — no numeric-ID branch, no
//!   substring matching, and the "Trophy ID" footer becomes the trophy name
//!   (UUIDs are never user-facing).
//!
//! The legacy easter-egg embed URL (details.js:36-42) is kept deliberately,
//! matching the choice made for `/show`.
//!
//! Business logic lives in plain testable functions; the handler stays thin.

use poise::serenity_prelude as serenity;

use crate::bot::{Context, Error, resolver, util};
use crate::entities::trophies;
use crate::i18n;

/// Legacy easter-egg embed URL, kept deliberately for parity (details.js:40).
const EMBED_URL: &str = "https://www.youtube.com/watch?v=PwP9ebvCBAM";

/// The displayable pieces of a `/details` reply, split out for testing.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct DetailsView {
    /// Embed title: `{emoji} {name}` (legacy details.js:37).
    pub title: String,
    /// Embed description: the private `details` field. The column is NOT
    /// NULL with a "No details provided." default, so there is nothing to
    /// fall back to here.
    pub details: String,
    /// Localized footer naming the trophy (replaces the legacy ID footer).
    pub footer: String,
}

/// Builds the reply pieces for a resolved trophy.
pub(crate) fn details_view(
    locale: &i18n::LanguageIdentifier,
    trophy: &trophies::Model,
) -> DetailsView {
    DetailsView {
        title: format!("{} {}", trophy.emoji, trophy.name),
        details: trophy.details.clone(),
        footer: i18n::t_args(
            locale,
            "details-footer",
            &[("name", trophy.name.clone().into())],
        ),
    }
}

/// Shows the details of a trophy.
#[poise::command(slash_command, guild_only, default_member_permissions = "MANAGE_GUILD")]
pub async fn details(
    ctx: Context<'_>,
    #[description = "Name of the trophy to show"]
    #[autocomplete = "resolver::autocomplete_trophy"]
    trophy: String,
) -> Result<(), Error> {
    let locale = util::locale(&ctx);
    let guild_id = ctx
        .guild_id()
        .ok_or_else(|| anyhow::anyhow!("guild_only command invoked outside a guild"))?;
    let db = &ctx.data().db;

    let Some(model) = resolver::resolve_trophy(db, guild_id.get() as i64, &trophy).await? else {
        return util::reply_error(
            ctx,
            i18n::t_args(&locale, "details-error-not-found", &[("input", trophy.into())]),
            true,
        )
        .await;
    };

    let view = details_view(&locale, &model);
    let embed = serenity::CreateEmbed::new()
        .title(view.title)
        .url(EMBED_URL)
        .description(view.details)
        .colour(util::COLOR_MAIN)
        .footer(serenity::CreateEmbedFooter::new(view.footer));

    // F11: private details stay private.
    util::reply_embed(ctx, embed, true).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use sea_orm::{ActiveModelTrait, DatabaseConnection, Set};
    use uuid::Uuid;

    use crate::domain::normalize::normalize_name;
    use crate::domain::test_support::{fresh_db, insert_guild, now};

    /// Inserts a trophy with a given details text, creating the guild row the
    /// FK needs, and returns the stored model.
    async fn insert_trophy(
        db: &DatabaseConnection,
        guild_id: i64,
        name: &str,
        details_text: &str,
    ) -> trophies::Model {
        insert_guild(db, guild_id).await;
        trophies::ActiveModel {
            id: Set(Uuid::now_v7()),
            guild_id: Set(guild_id),
            legacy_id: Set(None),
            creator_user_id: Set(None),
            name: Set(name.to_string()),
            normalized_name: Set(normalize_name(name)),
            description: Set("No description provided".to_string()),
            emoji: Set("🏆".to_string()),
            value: Set(10),
            image: Set(None),
            dedication_user_id: Set(None),
            dedication_text: Set(None),
            details: Set(details_text.to_string()),
            signed: Set(false),
            created_at: Set(now()),
            updated_at: Set(now()),
        }
        .insert(db)
        .await
        .expect("insert trophy")
    }

    // --- details_view ---

    #[tokio::test]
    async fn resolved_trophy_renders_title_details_and_name_footer() {
        // End-to-end through the shared resolver: fuzzy-cased input resolves
        // (F12) and the view shows the private details with a NAME footer.
        let db = fresh_db().await;
        insert_trophy(&db, 1, "Gold Medal", "Given for winning the raid.").await;

        let model = resolver::resolve_trophy(&db, 1, "gold-MEDAL!")
            .await
            .expect("query ok")
            .expect("trophy resolves");
        let view = details_view(&i18n::resolve(None), &model);

        assert_eq!(view.title, "🏆 Gold Medal");
        assert_eq!(view.details, "Given for winning the raid.");
        assert!(view.footer.contains("Gold Medal"), "footer: {}", view.footer);
        assert!(!view.footer.contains("ID"), "no ID in footer: {}", view.footer);
    }

    #[tokio::test]
    async fn default_details_text_is_shown_verbatim() {
        // Legacy default (details.js:32) is stored in the column at insert
        // time; the view passes it through untouched.
        let db = fresh_db().await;
        let model = insert_trophy(&db, 1, "Plain", "No details provided.").await;

        let view = details_view(&i18n::resolve(None), &model);
        assert_eq!(view.details, "No details provided.");
    }

    // --- i18n catalog ---

    #[test]
    fn all_details_messages_exist() {
        let locale = i18n::resolve(None);
        let args: &[(&'static str, i18n::FluentValue<'static>)] =
            &[("input", "Gold".into()), ("name", "Gold".into())];
        for key in ["details-error-not-found", "details-footer"] {
            assert_ne!(i18n::t_args(&locale, key, args), key, "missing ftl message: {key}");
        }
    }

    #[test]
    fn not_found_message_renders_the_input() {
        let locale = i18n::resolve(None);
        let message =
            i18n::t_args(&locale, "details-error-not-found", &[("input", "Golde".into())]);
        assert!(message.contains("Golde"), "got: {message}");
    }
}
