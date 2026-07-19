//! Component-interaction (button) handling, wired into poise through
//! `FrameworkOptions::event_handler` (see `src/bot/mod.rs`).
//!
//! Two confirmation flows live here: `/forgetme` (batch C16, F33) and the
//! destructive `/delete` (spec Rust target, rust-parity-plan §4), plus the
//! `/show` holders toggle. All are **stateless**: everything a press needs is
//! encoded in the button's custom id (`forgetme:{action}:{unix}` /
//! `trophy-delete:{action}:{unix}:{invoker}:{trophy_uuid}` /
//! `trophy-show-holders:{trophy_uuid}:{action}:{invoker}`), so a restart
//! needs no in-memory state at all. The confirmation flows additionally
//! reject a press more than [`CONFIRM_TIMEOUT_SECS`] after issuance — the
//! message loses its buttons and shows an "expired" notice instead of
//! acting. Legacy buttons never expired (and legacy /delete had no
//! confirmation at all); both are intentional, documented deltas
//! (rust-parity-plan.md §4) announced in the warning embeds themselves.
//!
//! Every press is acknowledged (`CreateInteractionResponse::Acknowledge`,
//! i.e. `DEFERRED_UPDATE_MESSAGE`) **before** any slow work — the owner
//! lookup can hit HTTP on a cache miss and the purge transaction can wait on
//! the SQLite writer — so the 3-second component-ack deadline can never be
//! breached ("no hanging interactions"). Later replies are therefore
//! `edit_response` (rewrites the button message) or ephemeral followups.
//!
//! Every error path here is handled inline (logged + best-effort ephemeral
//! reply): the handler always returns `Ok` so the framework's generic error
//! path (which has no interaction context for events) is never needed.

use std::path::Path;

use poise::serenity_prelude as serenity;
use sea_orm::{
    ColumnTrait, EntityTrait, QueryFilter, QueryOrder, QuerySelect, TransactionSession,
    TransactionTrait,
};
use uuid::Uuid;

use crate::bot::commands::{delete, show};
use crate::bot::{images, reward_apply, util, Data, Error};
use crate::entities::{guilds, trophies, user_trophies};
use crate::i18n;

/// How long the /forgetme and /delete confirmation buttons stay valid.
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

/// What a /forgetme button press must result in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PressOutcome {
    /// Presser is not the guild owner: ephemeral rejection, message untouched.
    RejectNotOwner,
    /// Confirmation is stale: disarm the message with an "expired" notice.
    Expired,
    /// Cancel pressed in time: replace the warning with a cancelled notice.
    Cancelled,
    /// Confirm pressed in time: purge everything and leave.
    Purge,
}

/// Pure decision for a /forgetme button press. Gate order matters: the owner
/// gate wins over expiry (non-owners never learn the message state), and the
/// expiry gate wins over both buttons.
pub(crate) fn press_outcome(
    button: ForgetmeButton,
    is_owner: bool,
    issued_at: i64,
    now: i64,
) -> PressOutcome {
    if !is_owner {
        return PressOutcome::RejectNotOwner;
    }
    if is_expired(issued_at, now) {
        return PressOutcome::Expired;
    }
    match button {
        ForgetmeButton::Cancel => PressOutcome::Cancelled,
        ForgetmeButton::Confirm => PressOutcome::Purge,
    }
}

// ---------------------------------------------------------------------------
// /delete confirmation flow (spec Rust target: "Add a confirmation button
// for destructive delete"; delta documented in rust-parity-plan.md §4)
// ---------------------------------------------------------------------------

/// Custom-id namespace for /show holders buttons.
const SHOW_HOLDERS_PREFIX: &str = "trophy-show-holders";

/// Whether the /show holders button should show or hide the extra section.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ShowHoldersAction {
    Show,
    Hide,
}

/// Builds the custom id for a /show holders button:
/// `trophy-show-holders:{trophy_uuid}:{show|hide}:{invoker_id}`.
pub(crate) fn show_holders_custom_id(
    trophy_id: Uuid,
    action: ShowHoldersAction,
    invoker_id: u64,
) -> String {
    let action_name = match action {
        ShowHoldersAction::Show => "show",
        ShowHoldersAction::Hide => "hide",
    };
    format!("{SHOW_HOLDERS_PREFIX}:{trophy_id}:{action_name}:{invoker_id}")
}

/// Parses a component custom id back into the trophy id, action, and
/// original invoker for the /show holders button.
pub(crate) fn parse_show_holders_custom_id(id: &str) -> Option<(Uuid, ShowHoldersAction, u64)> {
    let rest = id.strip_prefix(SHOW_HOLDERS_PREFIX)?.strip_prefix(':')?;
    let (trophy, rest) = rest.split_once(':')?;
    let (action, invoker_id) = rest.split_once(':')?;
    let action = match action {
        "show" => ShowHoldersAction::Show,
        "hide" => ShowHoldersAction::Hide,
        _ => return None,
    };
    Some((Uuid::parse_str(trophy).ok()?, action, invoker_id.parse().ok()?))
}

/// Custom-id namespace for /delete confirmation buttons.
const DELETE_PREFIX: &str = "trophy-delete";

/// The two buttons of the /delete confirmation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DeleteButton {
    Confirm,
    Cancel,
}

/// Builds the custom id for a /delete button:
/// `trophy-delete:{confirm|cancel}:{issued_at}:{invoker_id}:{trophy_uuid}`.
/// Everything a press needs is in the id (stateless, restart-safe); at 90
/// characters worst case it stays under Discord's 100-char custom-id cap.
pub(crate) fn delete_custom_id(
    button: DeleteButton,
    issued_at: i64,
    invoker_id: u64,
    trophy_id: Uuid,
) -> String {
    let action = match button {
        DeleteButton::Confirm => "confirm",
        DeleteButton::Cancel => "cancel",
    };
    format!("{DELETE_PREFIX}:{action}:{issued_at}:{invoker_id}:{trophy_id}")
}

/// Parses a component custom id back into a /delete button press. Returns
/// `None` for anything that is not exactly ours.
pub(crate) fn parse_delete_custom_id(id: &str) -> Option<(DeleteButton, i64, u64, Uuid)> {
    let rest = id.strip_prefix(DELETE_PREFIX)?.strip_prefix(':')?;
    let (action, rest) = rest.split_once(':')?;
    let button = match action {
        "confirm" => DeleteButton::Confirm,
        "cancel" => DeleteButton::Cancel,
        _ => return None,
    };
    let (issued_at, rest) = rest.split_once(':')?;
    let (invoker_id, trophy_id) = rest.split_once(':')?;
    Some((
        button,
        issued_at.parse().ok()?,
        invoker_id.parse().ok()?,
        Uuid::parse_str(trophy_id).ok()?,
    ))
}

/// What a /delete button press must result in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DeletePressOutcome {
    /// Presser is not the /delete invoker: ephemeral rejection, message kept.
    RejectNotInvoker,
    /// Confirmation is stale: disarm the message with an "expired" notice.
    Expired,
    /// Cancel pressed in time: replace the warning with a cancelled notice.
    Cancelled,
    /// Confirm pressed in time: hard-delete the trophy.
    Delete,
}

/// Pure decision for a /delete button press. Mirrors the /forgetme gate
/// order: the invoker gate wins over expiry (the confirmation is public, so
/// anyone in the channel can press — only the invoker may decide), and the
/// expiry gate wins over both buttons.
pub(crate) fn delete_press_outcome(
    button: DeleteButton,
    is_invoker: bool,
    issued_at: i64,
    now: i64,
) -> DeletePressOutcome {
    if !is_invoker {
        return DeletePressOutcome::RejectNotInvoker;
    }
    if is_expired(issued_at, now) {
        return DeletePressOutcome::Expired;
    }
    match button {
        DeleteButton::Cancel => DeletePressOutcome::Cancelled,
        DeleteButton::Confirm => DeletePressOutcome::Delete,
    }
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

    let result = if let Some((button, issued_at)) = parse_custom_id(&component.data.custom_id) {
        ("forgetme", handle_forgetme(ctx, component, data, button, issued_at).await)
    } else if let Some((trophy_id, action, invoker_id)) =
        parse_show_holders_custom_id(&component.data.custom_id)
    {
        (
            "show-holders",
            handle_show_holders(ctx, component, data, trophy_id, action, invoker_id).await,
        )
    } else if let Some((button, issued_at, invoker_id, trophy_id)) =
        parse_delete_custom_id(&component.data.custom_id)
    {
        (
            "delete",
            handle_trophy_delete(ctx, component, data, button, issued_at, invoker_id, trophy_id)
                .await,
        )
    } else {
        return Ok(()); // not one of ours
    };

    if let (flow, Err(err)) = result {
        let guild = component.guild_id.map(|g| g.get());
        let user = component.user.id.get();
        log::error!("{flow} button failed (guild={guild:?}, user={user}): {err:#}");
        let locale = i18n::resolve(Some(&component.locale));
        // The interaction was acknowledged first thing in the handler, so
        // errors are delivered as an ephemeral followup. If the ack itself
        // was what failed, this fails too and is only logged.
        let reply = ephemeral_followup(error_embed(
            &locale,
            i18n::t(&locale, "common-error-generic"),
        ));
        if let Err(reply_err) = component.create_followup(&ctx.http, reply).await {
            log::error!(
                "failed to deliver {flow} error reply (guild={guild:?}, user={user}): {reply_err}"
            );
        }
    }
    Ok(())
}

async fn handle_show_holders(
    ctx: &serenity::Context,
    component: &serenity::ComponentInteraction,
    data: &Data,
    trophy_id: Uuid,
    action: ShowHoldersAction,
    invoker_id: u64,
) -> anyhow::Result<()> {
    let locale = i18n::resolve(Some(&component.locale));
    let Some(guild_id) = component.guild_id else {
        return Ok(());
    };

    component
        .create_response(&ctx.http, serenity::CreateInteractionResponse::Acknowledge)
        .await?;

    if component.user.id.get() != invoker_id {
        let reply = ephemeral_followup(error_embed(
            &locale,
            i18n::t(&locale, "show-holders-not-invoker"),
        ));
        component.create_followup(&ctx.http, reply).await?;
        return Ok(());
    }

    let trophy = trophies::Entity::find_by_id(trophy_id)
        .filter(trophies::Column::GuildId.eq(guild_id.get() as i64))
        .one(&data.db)
        .await?;
    let Some(trophy) = trophy else {
        let embed = serenity::CreateEmbed::new()
            .title(i18n::t(&locale, "show-holders-missing-title"))
            .description(i18n::t(&locale, "show-holders-missing"))
            .colour(util::COLOR_ERROR);
        component
            .edit_response(&ctx.http, replace_message(embed, vec![]))
            .await?;
        return Ok(());
    };

    let dedication = trophy.dedication_user_id.map(|user_id| format!("<@{user_id}>"));
    let dedication = dedication.or_else(|| trophy.dedication_text.clone());

    let image = show::plan_image(Path::new(images::IMAGES_DIR), trophy.image.as_deref()).await;

    let mut embed = serenity::CreateEmbed::new()
        .title(format!("{} {}", trophy.emoji, trophy.name))
        .url("https://www.youtube.com/watch?v=04854XqcfCY")
        .description(trophy.description.clone())
        .colour(util::COLOR_MAIN)
        .field(
            i18n::t(&locale, "show-field-value"),
            i18n::t_args(&locale, "show-value", &[("value", i64::from(trophy.value).into())]),
            true,
        )
        .footer(serenity::CreateEmbedFooter::new(i18n::t_args(
            &locale,
            "show-footer",
            &[("name", trophy.name.clone().into())],
        )));

    if trophy.signed && let Some(creator) = trophy.creator_user_id {
        embed = embed.field(i18n::t(&locale, "show-field-signed"), format!("<@{creator}>"), true);
    }
    if let Some(dedicated_to) = dedication {
        embed = embed.field(i18n::t(&locale, "show-field-dedicated"), dedicated_to, true);
    }

    let mut embeds = vec![embed];
    let next_action = match action {
        ShowHoldersAction::Show => ShowHoldersAction::Hide,
        ShowHoldersAction::Hide => ShowHoldersAction::Show,
    };

    if action == ShowHoldersAction::Show {
        let holders = user_trophies::Entity::find()
            .filter(user_trophies::Column::GuildId.eq(guild_id.get() as i64))
            .filter(user_trophies::Column::TrophyId.eq(trophy_id))
            .order_by_desc(user_trophies::Column::AwardedAt)
            .all(&data.db)
            .await?;

        let lines = if holders.is_empty() {
            vec![i18n::t(&locale, "show-holders-empty")]
        } else {
            holders
                .into_iter()
                .map(|row| {
                    let user = row.user_id.to_string();
                    let when = row.awarded_at.format("%Y-%m-%d %H:%M").to_string();
                    format!("<@{user}> — {when}")
                })
                .collect::<Vec<_>>()
        };

        let holders_embed = serenity::CreateEmbed::new()
            .title(i18n::t(&locale, "show-holders-section-title"))
            .description(lines.join("\n"))
            .colour(util::COLOR_MAIN);
        embeds.push(holders_embed);
    }

    let button_row = serenity::CreateActionRow::Buttons(vec![serenity::CreateButton::new(
        show_holders_custom_id(trophy_id, next_action, invoker_id),
    )
    .style(serenity::ButtonStyle::Primary)
    .label(i18n::t(
        &locale,
        match next_action {
            ShowHoldersAction::Show => "show-button-holders",
            ShowHoldersAction::Hide => "show-button-hide-holders",
        },
    ))]);

    let mut edit = serenity::EditInteractionResponse::new()
        .embeds(embeds)
        .components(vec![button_row]);
    if let show::ImagePlan::Attachment { filename, bytes } = image {
        edit = edit.attachments(serenity::EditAttachments::new().add(
            serenity::CreateAttachment::bytes(bytes, filename),
        ));
    }

    component.edit_response(&ctx.http, edit).await?;
    Ok(())
}

/// The /delete confirmation flow: ack → invoker gate → expiry gate → cancel
/// or confirm (hard-delete the trophy, rewrite the message, remove its image,
/// recompute reward roles for every user that held it).
async fn handle_trophy_delete(
    ctx: &serenity::Context,
    component: &serenity::ComponentInteraction,
    data: &Data,
    button: DeleteButton,
    issued_at: i64,
    invoker_id: u64,
    trophy_id: Uuid,
) -> anyhow::Result<()> {
    let locale = i18n::resolve(Some(&component.locale));
    let Some(guild_id) = component.guild_id else {
        return Ok(()); // guild message components can't fire outside a guild
    };

    // Acknowledge FIRST (deferred update, no loading state): the delete
    // transaction can wait on the SQLite writer, so responding only
    // afterwards risks blowing the 3-second component-ack deadline (same
    // rationale as /forgetme).
    component
        .create_response(&ctx.http, serenity::CreateInteractionResponse::Acknowledge)
        .await?;

    let outcome = delete_press_outcome(
        button,
        component.user.id.get() == invoker_id,
        issued_at,
        chrono::Utc::now().timestamp(),
    );
    match outcome {
        DeletePressOutcome::RejectNotInvoker => {
            let reply =
                ephemeral_followup(error_embed(&locale, i18n::t(&locale, "delete-not-invoker")));
            component.create_followup(&ctx.http, reply).await?;
        }
        DeletePressOutcome::Expired => {
            let embed = serenity::CreateEmbed::new()
                .title(i18n::t(&locale, "delete-expired-title"))
                .description(i18n::t(&locale, "delete-expired"))
                .colour(util::COLOR_ERROR);
            component
                .edit_response(&ctx.http, replace_message(embed, vec![]))
                .await?;
        }
        DeletePressOutcome::Cancelled => {
            let embed = serenity::CreateEmbed::new()
                .title(i18n::t(&locale, "delete-cancelled-title"))
                .description(i18n::t(&locale, "delete-cancelled"))
                .colour(util::COLOR_MAIN);
            component
                .edit_response(&ctx.http, replace_message(embed, vec![]))
                .await?;
        }
        DeletePressOutcome::Delete => {
            // Re-load the trophy scoped to THIS guild: it may already be
            // gone (another manager, a stale message) and a custom id can
            // never delete across guilds.
            let trophy = trophies::Entity::find_by_id(trophy_id)
                .filter(trophies::Column::GuildId.eq(guild_id.get() as i64))
                .one(&data.db)
                .await?;
            let Some(trophy) = trophy else {
                let embed = serenity::CreateEmbed::new()
                    .title(i18n::t(&locale, "delete-gone-title"))
                    .description(i18n::t(&locale, "delete-gone"))
                    .colour(util::COLOR_ERROR);
                component
                    .edit_response(&ctx.http, replace_message(embed, vec![]))
                    .await?;
                return Ok(());
            };

            // 1. Hard delete (FK cascade wipes the awards) — F10 semantics,
            //    shared with the old direct path.
            let affected = delete::delete_trophy(&data.db, trophy.id).await?;

            // 2. Rewrite the confirmation with the success embed. The delete
            //    is committed, so a failed edit must NOT abort the flow:
            //    image cleanup and reward recompute below run regardless.
            let embed = serenity::CreateEmbed::new()
                .colour(util::COLOR_SUCCESS)
                .description(i18n::t_args(
                    &locale,
                    "delete-success",
                    &[
                        ("emoji", trophy.emoji.clone().into()),
                        ("name", trophy.name.clone().into()),
                    ],
                ));
            if let Err(err) = component
                .edit_response(&ctx.http, replace_message(embed, vec![]))
                .await
            {
                log::error!(
                    "delete: failed to acknowledge deletion of trophy {} (guild={}): {err}",
                    trophy.id,
                    guild_id.get()
                );
            }

            // 3. F10: unlink the image only when the trophy actually has one
            //    (`remove` logs failures instead of swallowing them).
            if let Some(image) = &trophy.image {
                images::remove(image).await;
            }

            // 4. §2 reward engine: awaited, idempotent, per-user failures
            //    logged — an engine hiccup after the committed delete must
            //    not stop the remaining users from recomputing.
            let bot_id = ctx.cache.current_user().id;
            for user_id in affected {
                if let Err(err) = reward_apply::apply_rewards_via(
                    &data.db,
                    ctx,
                    bot_id,
                    None,
                    guild_id,
                    serenity::UserId::new(user_id as u64),
                )
                .await
                {
                    log::error!(
                        "reward recompute failed after /delete (guild={}, user={user_id}): {err:#}",
                        guild_id.get()
                    );
                }
            }
            log::info!(
                "delete: trophy {} removed from guild {} on confirmation by invoker {}",
                trophy.id,
                guild_id.get(),
                invoker_id
            );
        }
    }
    Ok(())
}

/// The /forgetme confirmation flow: ack → owner gate → expiry gate → cancel
/// or confirm (purge DB, rewrite the message, remove images, leave the guild).
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

    // Acknowledge FIRST (deferred update, no loading state): the owner lookup
    // below can cost an HTTP round trip on a cache miss and the purge can
    // wait on the SQLite writer (5s busy_timeout), so responding only
    // afterwards risks blowing the 3-second component-ack deadline — the
    // guild would be purged while the owner sees "This interaction failed".
    component
        .create_response(&ctx.http, serenity::CreateInteractionResponse::Acknowledge)
        .await?;

    // Owner gate at press time too (anyone in the channel can see the button).
    // The cache guard is dropped within its own statement.
    let cached_owner = guild_id.to_guild_cached(&ctx.cache).map(|g| g.owner_id);
    let owner_id = match cached_owner {
        Some(owner_id) => owner_id,
        None => ctx.http.get_guild(guild_id).await?.owner_id,
    };

    let outcome = press_outcome(
        button,
        component.user.id == owner_id,
        issued_at,
        chrono::Utc::now().timestamp(),
    );
    match outcome {
        PressOutcome::RejectNotOwner => {
            let reply =
                ephemeral_followup(error_embed(&locale, i18n::t(&locale, "forgetme-not-owner")));
            component.create_followup(&ctx.http, reply).await?;
        }
        PressOutcome::Expired => {
            // Disarm the stale message: swap the warning for an expired
            // notice and drop the buttons.
            let embed = serenity::CreateEmbed::new()
                .title(i18n::t(&locale, "forgetme-expired-title"))
                .description(i18n::t(&locale, "forgetme-expired"))
                .colour(util::COLOR_ERROR);
            component
                .edit_response(&ctx.http, replace_message(embed, vec![]))
                .await?;
        }
        PressOutcome::Cancelled => {
            let embed = serenity::CreateEmbed::new()
                .title(i18n::t(&locale, "forgetme-cancelled-title"))
                .description(i18n::t(&locale, "forgetme-cancelled"))
                .colour(util::COLOR_MAIN);
            component
                .edit_response(&ctx.http, replace_message(embed, vec![]))
                .await?;
        }
        PressOutcome::Purge => {
            // 1. True cascade delete inside one transaction. Errors propagate
            //    (the interaction is already acknowledged; the caller answers
            //    the user with an ephemeral followup, the buttons stay live).
            let image_files = purge_guild_data(&data.db, guild_id.get() as i64).await?;

            // 2. Rewrite the message BEFORE leaving the guild, replacing the
            //    warning (and its buttons) with the goodbye. The data is
            //    already gone, so a failed edit must NOT abort the flow:
            //    image cleanup and the guild leave below run regardless.
            let embed = serenity::CreateEmbed::new()
                .title(i18n::t(&locale, "forgetme-goodbye-title"))
                .description(i18n::t(&locale, "forgetme-goodbye"))
                .colour(util::COLOR_MAIN);
            if let Err(err) = component
                .edit_response(&ctx.http, replace_message(embed, vec![]))
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
                images::remove(image).await;
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

/// Ephemeral single-embed followup (the interaction is always acknowledged
/// with a deferred update first, so plain responses are no longer possible).
fn ephemeral_followup(embed: serenity::CreateEmbed) -> serenity::CreateInteractionResponseFollowup {
    serenity::CreateInteractionResponseFollowup::new()
        .embed(embed)
        .ephemeral(true)
}

/// Edit that replaces the button message with `embed` and strips all
/// components (follows the initial `Acknowledge` deferred update).
fn replace_message(
    embed: serenity::CreateEmbed,
    components: Vec<serenity::CreateActionRow>,
) -> serenity::EditInteractionResponse {
    serenity::EditInteractionResponse::new().embed(embed).components(components)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sea_orm::{ActiveModelTrait, DatabaseConnection, Set};
    use uuid::Uuid;

    #[test]
    fn show_holders_toggle_custom_id_round_trips() {
        let trophy = Uuid::now_v7();
        assert_eq!(
            parse_show_holders_custom_id(&show_holders_custom_id(
                trophy,
                ShowHoldersAction::Show,
                42
            )),
            Some((trophy, ShowHoldersAction::Show, 42))
        );
        assert_eq!(
            parse_show_holders_custom_id(&show_holders_custom_id(
                trophy,
                ShowHoldersAction::Hide,
                42
            )),
            Some((trophy, ShowHoldersAction::Hide, 42))
        );
    }

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

    /// The button expiry is a deliberate divergence from legacy (whose
    /// `forgetmeproceed` button stayed live forever) and MUST stay listed in
    /// the intentional-deltas catalog with the actual timeout value. If this
    /// test fails you changed the behavior (or the constant) without updating
    /// docs/specs/rust-parity-plan.md §4.
    #[test]
    fn expiry_delta_is_documented_in_the_parity_plan() {
        let plan = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/docs/specs/rust-parity-plan.md"
        ))
        .expect("read rust-parity-plan.md");
        let deltas = plan
            .split("## 4.")
            .nth(1)
            .expect("parity plan has a §4 intentional-deltas section");
        assert!(
            deltas.contains("`/forgetme` confirmation buttons expire"),
            "§4 must list the /forgetme button-expiry delta"
        );
        assert!(
            deltas.contains(&format!("{CONFIRM_TIMEOUT_SECS} seconds")),
            "§4 must state the current timeout ({CONFIRM_TIMEOUT_SECS} seconds)"
        );
    }

    // --- press_outcome ---

    #[test]
    fn non_owner_is_rejected_before_anything_else() {
        // Even an expired press must not reveal message state to non-owners.
        for button in [ForgetmeButton::Confirm, ForgetmeButton::Cancel] {
            for now in [1_000, 1_000 + CONFIRM_TIMEOUT_SECS + 1] {
                assert_eq!(
                    press_outcome(button, false, 1_000, now),
                    PressOutcome::RejectNotOwner
                );
            }
        }
    }

    #[test]
    fn stale_owner_press_expires_regardless_of_button() {
        let now = 1_000 + CONFIRM_TIMEOUT_SECS + 1;
        for button in [ForgetmeButton::Confirm, ForgetmeButton::Cancel] {
            assert_eq!(
                press_outcome(button, true, 1_000, now),
                PressOutcome::Expired
            );
        }
    }

    #[test]
    fn fresh_owner_press_maps_confirm_to_purge_and_cancel_to_cancelled() {
        assert_eq!(
            press_outcome(ForgetmeButton::Confirm, true, 1_000, 1_000 + CONFIRM_TIMEOUT_SECS),
            PressOutcome::Purge
        );
        assert_eq!(
            press_outcome(ForgetmeButton::Cancel, true, 1_000, 1_000),
            PressOutcome::Cancelled
        );
    }

    /// Regression guard for the interaction-timeout fix: `handle_forgetme`
    /// must acknowledge the component interaction BEFORE the (potentially
    /// slow) HTTP owner fetch and the purge transaction, so a >3s purge can
    /// never leave the owner with "This interaction failed" on an already
    /// purged guild. Serenity types aren't mockable here, so this checks the
    /// statement order in the source itself.
    #[test]
    fn handler_acknowledges_before_owner_fetch_and_purge() {
        let src = include_str!("buttons.rs");
        let handler = src
            .split("async fn handle_forgetme")
            .nth(1)
            .expect("handle_forgetme exists");
        let ack = handler
            .find("CreateInteractionResponse::Acknowledge")
            .expect("handler must acknowledge the interaction");
        let owner_fetch = handler
            .find("get_guild(guild_id)")
            .expect("handler fetches the owner on cache miss");
        let purge = handler
            .find("purge_guild_data(&data.db")
            .expect("handler purges on confirm");
        assert!(
            ack < owner_fetch && ack < purge,
            "the interaction must be acknowledged before the owner fetch \
             ({ack} vs {owner_fetch}) and before the purge ({ack} vs {purge})"
        );
    }

    // --- /delete confirmation flow ---

    #[test]
    fn delete_custom_id_round_trips_both_buttons() {
        let trophy = Uuid::now_v7();
        for button in [DeleteButton::Confirm, DeleteButton::Cancel] {
            let id = delete_custom_id(button, 1_751_900_000, 42, trophy);
            assert!(id.len() <= 100, "custom id must fit Discord's cap: {id}");
            assert_eq!(
                parse_delete_custom_id(&id),
                Some((button, 1_751_900_000, 42, trophy))
            );
        }
    }

    #[test]
    fn delete_parse_rejects_foreign_and_malformed_ids() {
        let trophy = Uuid::now_v7();
        let ids = [
            String::new(),
            "trophy-delete".to_string(),
            "trophy-delete:".to_string(),
            "trophy-delete:confirm".to_string(),
            "trophy-delete:confirm:123".to_string(),
            "trophy-delete:confirm:123:42".to_string(),
            "trophy-delete:confirm:123:42:not-a-uuid".to_string(),
            format!("trophy-delete:nuke:123:42:{trophy}"),
            format!("trophy-delete:confirm:abc:42:{trophy}"),
            format!("trophy-delete:confirm:123:abc:{trophy}"),
            format!("other:confirm:123:42:{trophy}"),
            "forgetme:confirm:123".to_string(), // the sibling flow must NOT match
        ];
        for id in &ids {
            assert_eq!(parse_delete_custom_id(id), None, "id {id:?} must not parse");
        }
        // And the forgetme parser must not eat delete ids either.
        let delete_id = delete_custom_id(DeleteButton::Confirm, 123, 42, trophy);
        assert_eq!(parse_custom_id(&delete_id), None);
    }

    #[test]
    fn delete_non_invoker_is_rejected_before_anything_else() {
        for button in [DeleteButton::Confirm, DeleteButton::Cancel] {
            for now in [1_000, 1_000 + CONFIRM_TIMEOUT_SECS + 1] {
                assert_eq!(
                    delete_press_outcome(button, false, 1_000, now),
                    DeletePressOutcome::RejectNotInvoker
                );
            }
        }
    }

    #[test]
    fn delete_stale_invoker_press_expires_regardless_of_button() {
        let now = 1_000 + CONFIRM_TIMEOUT_SECS + 1;
        for button in [DeleteButton::Confirm, DeleteButton::Cancel] {
            assert_eq!(
                delete_press_outcome(button, true, 1_000, now),
                DeletePressOutcome::Expired
            );
        }
    }

    #[test]
    fn delete_fresh_invoker_press_maps_confirm_and_cancel() {
        assert_eq!(
            delete_press_outcome(DeleteButton::Confirm, true, 1_000, 1_000 + CONFIRM_TIMEOUT_SECS),
            DeletePressOutcome::Delete
        );
        assert_eq!(
            delete_press_outcome(DeleteButton::Cancel, true, 1_000, 1_000),
            DeletePressOutcome::Cancelled
        );
    }

    /// Same regression guard as the /forgetme one: the /delete press handler
    /// must acknowledge the component interaction BEFORE the trophy lookup
    /// and the delete transaction.
    #[test]
    fn delete_handler_acknowledges_before_lookup_and_delete() {
        let src = include_str!("buttons.rs");
        let handler = src
            .split("async fn handle_trophy_delete")
            .nth(1)
            .expect("handle_trophy_delete exists");
        let ack = handler
            .find("CreateInteractionResponse::Acknowledge")
            .expect("handler must acknowledge the interaction");
        let lookup = handler
            .find("trophies::Entity::find_by_id")
            .expect("handler re-loads the trophy");
        let delete = handler
            .find("delete::delete_trophy(")
            .expect("handler deletes on confirm");
        assert!(
            ack < lookup && ack < delete,
            "the interaction must be acknowledged before the trophy lookup \
             ({ack} vs {lookup}) and before the delete ({ack} vs {delete})"
        );
    }

    /// The /delete confirmation (legacy had none) is a user-visible delta
    /// and MUST stay listed in the intentional-deltas catalog, including the
    /// expiry window. If this fails you changed the flow (or the constant)
    /// without updating docs/specs/rust-parity-plan.md §4.
    #[test]
    fn delete_confirmation_delta_is_documented_in_the_parity_plan() {
        let plan = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/docs/specs/rust-parity-plan.md"
        ))
        .expect("read rust-parity-plan.md");
        let deltas = plan
            .split("## 4.")
            .nth(1)
            .expect("parity plan has a §4 intentional-deltas section");
        assert!(
            deltas.contains("`/delete` asks for confirmation"),
            "§4 must list the /delete confirmation delta"
        );
        assert!(
            deltas.contains(&format!("{CONFIRM_TIMEOUT_SECS} seconds")),
            "§4 must state the current timeout ({CONFIRM_TIMEOUT_SECS} seconds)"
        );
    }

    #[test]
    fn delete_flow_catalog_messages_exist() {
        let locale = i18n::resolve(None);
        for key in [
            "delete-not-invoker",
            "delete-cancelled-title",
            "delete-cancelled",
            "delete-expired-title",
            "delete-expired",
            "delete-gone-title",
            "delete-gone",
        ] {
            assert_ne!(i18n::t(&locale, key), key, "missing Fluent key {key}");
        }
    }

    #[test]
    fn show_holders_flow_catalog_messages_exist() {
        let locale = i18n::resolve(None);
        for key in [
            "show-holders-not-invoker",
            "show-holders-missing-title",
            "show-holders-missing",
            "show-holders-empty",
            "show-holders-section-title",
        ] {
            assert_ne!(i18n::t(&locale, key), key, "missing Fluent key {key}");
        }
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
