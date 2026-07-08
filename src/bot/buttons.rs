//! Component-interaction (button) handling, wired into poise through
//! `FrameworkOptions::event_handler` (see `src/bot/mod.rs`).
//!
//! The only component flow today is the `/forgetme` confirmation (batch C16,
//! F33). The flow is **stateless**: the issue timestamp is encoded in each
//! button's custom id (`forgetme:confirm:{unix}` / `forgetme:cancel:{unix}`),
//! and a press more than [`CONFIRM_TIMEOUT_SECS`] after issuance is rejected
//! — the message loses its buttons and shows an "expired" notice instead of
//! acting. No in-memory state survives a restart or is needed at all.
//!
//! Every error path here is handled inline (logged + best-effort ephemeral
//! reply): the handler always returns `Ok` so the framework's generic error
//! path (which has no interaction context for events) is never needed.

use poise::serenity_prelude as serenity;
use sea_orm::{
    ColumnTrait, EntityTrait, QueryFilter, QuerySelect, TransactionSession, TransactionTrait,
};

use crate::bot::{images, util, Data, Error};
use crate::entities::{guilds, trophies};
use crate::i18n;

/// How long the /forgetme confirmation buttons stay valid.
pub(crate) const CONFIRM_TIMEOUT_SECS: i64 = 60;

/// Custom-id namespace for /forgetme buttons.
const FORGETME_PREFIX: &str = "forgetme";

/// The two buttons of the /forgetme confirmation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ForgetmeButton {
    Confirm,
    Cancel,
}

/// Builds the custom id for a /forgetme button issued at `issued_at`
/// (unix seconds): `forgetme:{confirm|cancel}:{issued_at}`.
pub(crate) fn custom_id(button: ForgetmeButton, issued_at: i64) -> String {
    let action = match button {
        ForgetmeButton::Confirm => "confirm",
        ForgetmeButton::Cancel => "cancel",
    };
    format!("{FORGETME_PREFIX}:{action}:{issued_at}")
}

/// Parses a component custom id back into a /forgetme button. Returns `None`
/// for anything that is not exactly ours (other flows just fall through).
pub(crate) fn parse_custom_id(id: &str) -> Option<(ForgetmeButton, i64)> {
    let rest = id.strip_prefix(FORGETME_PREFIX)?.strip_prefix(':')?;
    let (action, issued_at) = rest.split_once(':')?;
    let button = match action {
        "confirm" => ForgetmeButton::Confirm,
        "cancel" => ForgetmeButton::Cancel,
        _ => return None,
    };
    Some((button, issued_at.parse().ok()?))
}

/// Whether a confirmation issued at `issued_at` is stale at `now`
/// (both unix seconds).
pub(crate) fn is_expired(issued_at: i64, now: i64) -> bool {
    now.saturating_sub(issued_at) > CONFIRM_TIMEOUT_SECS
}

/// Deletes ALL data of `guild_id` in one transaction and returns the image
/// filenames its trophies referenced (for filesystem cleanup by the caller).
///
/// A single `DELETE FROM guilds` suffices: every child table (trophies →
/// user_trophies, guild_settings, role_rewards, leaderboard_panels) hangs off
/// `guilds.id` with `ON DELETE CASCADE` (schema.md). The image filenames must
/// be collected in the same transaction, BEFORE the cascade wipes the rows.
/// True delete — no legacy `-1` tombstone (F33).
pub(crate) async fn purge_guild_data<C: TransactionTrait>(
    db: &C,
    guild_id: i64,
) -> anyhow::Result<Vec<String>> {
    let txn = db.begin().await?;
    let image_files: Vec<String> = trophies::Entity::find()
        .filter(trophies::Column::GuildId.eq(guild_id))
        .filter(trophies::Column::Image.is_not_null())
        .select_only()
        .column(trophies::Column::Image)
        .into_tuple()
        .all(&txn)
        .await?;
    guilds::Entity::delete_by_id(guild_id).exec(&txn).await?;
    txn.commit().await?;
    Ok(image_files)
}

/// Poise `event_handler` entry point: dispatches component interactions we
/// recognize and ignores everything else.
pub async fn handle_event(
    ctx: &serenity::Context,
    event: &serenity::FullEvent,
    _framework: poise::FrameworkContext<'_, Data, Error>,
    data: &Data,
) -> Result<(), Error> {
    let serenity::FullEvent::InteractionCreate {
        interaction: serenity::Interaction::Component(component),
    } = event
    else {
        return Ok(());
    };
    let Some((button, issued_at)) = parse_custom_id(&component.data.custom_id) else {
        return Ok(()); // not one of ours
    };

    if let Err(err) = handle_forgetme(ctx, component, data, button, issued_at).await {
        let guild = component.guild_id.map(|g| g.get());
        let user = component.user.id.get();
        log::error!("forgetme button failed (guild={guild:?}, user={user}): {err:#}");
        let locale = i18n::resolve(Some(&component.locale));
        let reply = ephemeral_embed(error_embed(
            &locale,
            i18n::t(&locale, "common-error-generic"),
        ));
        if let Err(reply_err) = component.create_response(&ctx.http, reply).await {
            log::error!(
                "failed to deliver forgetme error reply (guild={guild:?}, user={user}): {reply_err}"
            );
        }
    }
    Ok(())
}

/// The /forgetme confirmation flow: owner gate → expiry gate → cancel or
/// confirm (purge DB, respond, remove images, leave the guild).
async fn handle_forgetme(
    ctx: &serenity::Context,
    component: &serenity::ComponentInteraction,
    data: &Data,
    button: ForgetmeButton,
    issued_at: i64,
) -> anyhow::Result<()> {
    let locale = i18n::resolve(Some(&component.locale));
    let Some(guild_id) = component.guild_id else {
        return Ok(()); // guild message components can't fire outside a guild
    };

    // Owner gate at press time too (anyone in the channel can see the button).
    // The cache guard is dropped within its own statement.
    let cached_owner = guild_id.to_guild_cached(&ctx.cache).map(|g| g.owner_id);
    let owner_id = match cached_owner {
        Some(owner_id) => owner_id,
        None => ctx.http.get_guild(guild_id).await?.owner_id,
    };
    if component.user.id != owner_id {
        let reply = ephemeral_embed(error_embed(&locale, i18n::t(&locale, "forgetme-not-owner")));
        component.create_response(&ctx.http, reply).await?;
        return Ok(());
    }

    if is_expired(issued_at, chrono::Utc::now().timestamp()) {
        // Disarm the stale message: swap the warning for an expired notice
        // and drop the buttons.
        let embed = serenity::CreateEmbed::new()
            .title(i18n::t(&locale, "forgetme-expired-title"))
            .description(i18n::t(&locale, "forgetme-expired"))
            .colour(util::COLOR_ERROR);
        component
            .create_response(&ctx.http, update_message(embed))
            .await?;
        return Ok(());
    }

    match button {
        ForgetmeButton::Cancel => {
            let embed = serenity::CreateEmbed::new()
                .title(i18n::t(&locale, "forgetme-cancelled-title"))
                .description(i18n::t(&locale, "forgetme-cancelled"))
                .colour(util::COLOR_MAIN);
            component
                .create_response(&ctx.http, update_message(embed))
                .await?;
        }
        ForgetmeButton::Confirm => {
            // 1. True cascade delete inside one transaction. Errors propagate
            //    (nothing was acknowledged yet, the caller answers the user).
            let image_files = purge_guild_data(&data.db, guild_id.get() as i64).await?;

            // 2. Acknowledge BEFORE leaving the guild, replacing the warning
            //    (and its buttons) with the goodbye. The data is already gone,
            //    so a failed acknowledgement must NOT abort the flow: image
            //    cleanup and the guild leave below run regardless.
            let embed = serenity::CreateEmbed::new()
                .title(i18n::t(&locale, "forgetme-goodbye-title"))
                .description(i18n::t(&locale, "forgetme-goodbye"))
                .colour(util::COLOR_MAIN);
            if let Err(err) = component
                .create_response(&ctx.http, update_message(embed))
                .await
            {
                log::error!(
                    "forgetme: failed to acknowledge purge of guild {}: {err}",
                    guild_id.get()
                );
            }

            // 3. Best-effort image cleanup; `images::remove` logs failures
            //    instead of swallowing them (fixes the legacy no-op unlink).
            for image in &image_files {
                images::remove(image);
            }

            // 4. Leave. Data is already gone; a failed leave is only logged.
            if let Err(err) = guild_id.leave(&ctx.http).await {
                log::error!(
                    "forgetme: failed to leave guild {} after purge: {err}",
                    guild_id.get()
                );
            }
            log::info!(
                "forgetme: purged guild {} ({} image file(s)) on request of owner {}",
                guild_id.get(),
                image_files.len(),
                component.user.id.get()
            );
        }
    }
    Ok(())
}

/// Error-styled embed with the shared localized error title.
fn error_embed(
    locale: &i18n::LanguageIdentifier,
    description: String,
) -> serenity::CreateEmbed {
    serenity::CreateEmbed::new()
        .title(i18n::t(locale, "common-error-title"))
        .description(description)
        .colour(util::COLOR_ERROR)
}

/// Ephemeral single-embed component response.
fn ephemeral_embed(embed: serenity::CreateEmbed) -> serenity::CreateInteractionResponse {
    serenity::CreateInteractionResponse::Message(
        serenity::CreateInteractionResponseMessage::new()
            .embed(embed)
            .ephemeral(true),
    )
}

/// Replaces the button message with `embed` and strips all components.
fn update_message(embed: serenity::CreateEmbed) -> serenity::CreateInteractionResponse {
    serenity::CreateInteractionResponse::UpdateMessage(
        serenity::CreateInteractionResponseMessage::new()
            .embed(embed)
            .components(vec![]),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use sea_orm::{ActiveModelTrait, DatabaseConnection, Set};
    use uuid::Uuid;

    use crate::domain::normalize::normalize_name;
    use crate::domain::test_support::{fresh_db, insert_guild, now};
    use crate::entities::{guild_settings, leaderboard_panels, role_rewards, user_trophies};

    // --- custom id round trip ---

    #[test]
    fn custom_id_round_trips_both_buttons() {
        for button in [ForgetmeButton::Confirm, ForgetmeButton::Cancel] {
            let id = custom_id(button, 1_751_900_000);
            assert_eq!(parse_custom_id(&id), Some((button, 1_751_900_000)));
        }
    }

    #[test]
    fn parse_rejects_foreign_and_malformed_ids() {
        for id in [
            "",
            "forgetme",
            "forgetme:",
            "forgetme:confirm",
            "forgetme:confirm:",
            "forgetme:confirm:notanumber",
            "forgetme:nuke:123",
            "other:confirm:123",
            "forgetmeproceed", // the legacy custom id must NOT match
        ] {
            assert_eq!(parse_custom_id(id), None, "id {id:?} must not parse");
        }
    }

    // --- expiry ---

    #[test]
    fn confirmation_expires_strictly_after_the_timeout() {
        let issued = 1_000;
        assert!(!is_expired(issued, issued), "fresh press is valid");
        assert!(
            !is_expired(issued, issued + CONFIRM_TIMEOUT_SECS),
            "press exactly at the deadline is still valid"
        );
        assert!(is_expired(issued, issued + CONFIRM_TIMEOUT_SECS + 1));
    }

    // --- purge_guild_data ---

    async fn insert_trophy(
        db: &DatabaseConnection,
        guild_id: i64,
        name: &str,
        image: Option<&str>,
    ) -> trophies::Model {
        trophies::ActiveModel {
            id: Set(Uuid::now_v7()),
            guild_id: Set(guild_id),
            legacy_id: Set(None),
            creator_user_id: Set(None),
            name: Set(name.to_string()),
            normalized_name: Set(normalize_name(name)),
            description: Set("d".into()),
            emoji: Set("🏆".into()),
            value: Set(10),
            image: Set(image.map(str::to_string)),
            dedication_user_id: Set(None),
            dedication_text: Set(None),
            details: Set("d".into()),
            signed: Set(false),
            created_at: Set(now()),
            updated_at: Set(now()),
        }
        .insert(db)
        .await
        .expect("insert trophy")
    }

    /// Seeds one row in every child table of `guilds` for `guild_id`.
    async fn seed_full_guild(db: &DatabaseConnection, guild_id: i64, image: Option<&str>) {
        insert_guild(db, guild_id).await;
        let trophy = insert_trophy(db, guild_id, "Seeded", image).await;
        user_trophies::ActiveModel {
            id: Set(Uuid::now_v7()),
            guild_id: Set(guild_id),
            user_id: Set(42),
            trophy_id: Set(trophy.id),
            awarded_by: Set(None),
            awarded_at: Set(now()),
            created_at: Set(now()),
            updated_at: Set(now()),
        }
        .insert(db)
        .await
        .expect("insert award");
        guild_settings::ActiveModel {
            guild_id: Set(guild_id),
            dedication_display: Set(Some(1)),
            stack_roles: Set(None),
            hide_unused_trophies: Set(None),
            hide_quit_users: Set(None),
            leaderboard_format: Set(None),
            created_at: Set(now()),
            updated_at: Set(now()),
        }
        .insert(db)
        .await
        .expect("insert settings");
        role_rewards::ActiveModel {
            id: Set(Uuid::now_v7()),
            guild_id: Set(guild_id),
            role_id: Set(500),
            requirement: Set(10),
            created_at: Set(now()),
            updated_at: Set(now()),
        }
        .insert(db)
        .await
        .expect("insert reward");
        leaderboard_panels::ActiveModel {
            guild_id: Set(guild_id),
            channel_id: Set(600),
            message_id: Set(700),
            created_at: Set(now()),
            updated_at: Set(now()),
        }
        .insert(db)
        .await
        .expect("insert panel");
    }

    #[tokio::test]
    async fn purge_deletes_the_guild_and_every_child_row() {
        let db = fresh_db().await;
        seed_full_guild(&db, 1, Some("1_img.png")).await;

        let image_files = purge_guild_data(&db, 1).await.unwrap();
        assert_eq!(image_files, vec!["1_img.png".to_string()]);

        assert!(
            guilds::Entity::find_by_id(1).one(&db).await.unwrap().is_none(),
            "true delete, not a tombstone"
        );
        assert!(trophies::Entity::find().all(&db).await.unwrap().is_empty());
        assert!(user_trophies::Entity::find().all(&db).await.unwrap().is_empty());
        assert!(guild_settings::Entity::find().all(&db).await.unwrap().is_empty());
        assert!(role_rewards::Entity::find().all(&db).await.unwrap().is_empty());
        assert!(leaderboard_panels::Entity::find().all(&db).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn purge_returns_only_non_null_image_filenames() {
        let db = fresh_db().await;
        insert_guild(&db, 1).await;
        insert_trophy(&db, 1, "NoImage", None).await;
        insert_trophy(&db, 1, "WithImage", Some("1_a.png")).await;
        insert_trophy(&db, 1, "AlsoImage", Some("1_b.gif")).await;

        let mut image_files = purge_guild_data(&db, 1).await.unwrap();
        image_files.sort();
        assert_eq!(image_files, vec!["1_a.png".to_string(), "1_b.gif".to_string()]);
    }

    #[tokio::test]
    async fn purge_leaves_other_guilds_untouched() {
        let db = fresh_db().await;
        seed_full_guild(&db, 1, Some("1_img.png")).await;
        seed_full_guild(&db, 2, Some("2_img.png")).await;

        let image_files = purge_guild_data(&db, 1).await.unwrap();
        assert_eq!(image_files, vec!["1_img.png".to_string()]);

        assert!(guilds::Entity::find_by_id(2).one(&db).await.unwrap().is_some());
        assert_eq!(trophies::Entity::find().all(&db).await.unwrap().len(), 1);
        assert_eq!(user_trophies::Entity::find().all(&db).await.unwrap().len(), 1);
        assert_eq!(guild_settings::Entity::find().all(&db).await.unwrap().len(), 1);
        assert_eq!(role_rewards::Entity::find().all(&db).await.unwrap().len(), 1);
        assert_eq!(
            leaderboard_panels::Entity::find().all(&db).await.unwrap().len(),
            1
        );
    }

    #[tokio::test]
    async fn purge_of_an_unknown_guild_is_a_no_op() {
        let db = fresh_db().await;
        seed_full_guild(&db, 1, None).await;

        let image_files = purge_guild_data(&db, 999).await.unwrap();
        assert!(image_files.is_empty());
        assert!(guilds::Entity::find_by_id(1).one(&db).await.unwrap().is_some());
    }
}
