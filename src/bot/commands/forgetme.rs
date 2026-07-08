//! `/forgetme` — owner-only, two-step deletion of all guild data (batch C16).
//!
//! Spec: docs/specs/commands-utility.md §/forgetme. Parity fixes (F33):
//! - Non-owners get an ephemeral localized rejection instead of the legacy
//!   silent reply delete.
//! - The confirmation gets a **real Cancel button** (the legacy flow rendered
//!   only the danger button; its `forgetmenope` handler was dead code).
//! - On confirm the guild row is **truly deleted** (FK `ON DELETE CASCADE`
//!   wipes trophies → user_trophies, settings, rewards and panels) instead of
//!   the legacy `-1` tombstone; image files are removed best-effort with
//!   logged errors; then the bot leaves the guild.
//!
//! The confirmation buttons are consumed by `src/bot/buttons.rs` (component
//! interaction handler wired into the poise framework options), which also
//! enforces the 60-second confirmation timeout encoded in the custom ids —
//! an intentional delta vs legacy (never-expiring buttons), documented in
//! rust-parity-plan.md §4.8 and announced in the warning embed below.

use poise::serenity_prelude as serenity;

use crate::bot::{buttons, util, Context, Error};
use crate::i18n;

/// Remove all images and data about your server from the bot and kick it.
#[poise::command(slash_command, guild_only, default_member_permissions = "ADMINISTRATOR", required_permissions = "ADMINISTRATOR")]
pub async fn forgetme(ctx: Context<'_>) -> Result<(), Error> {
    let locale = util::locale(&ctx);
    let guild_id = util::require_guild_id(&ctx)?;

    // Owner gate (F33: explicit ephemeral rejection, never a silent no-op).
    // Cache first; the guard is dropped at the end of the statement so it
    // never lives across an await.
    let cached_owner = ctx.guild().map(|guild| guild.owner_id);
    let owner_id = match cached_owner {
        Some(owner_id) => owner_id,
        None => ctx.http().get_guild(guild_id).await?.owner_id,
    };
    if ctx.author().id != owner_id {
        return util::reply_error(ctx, i18n::t(&locale, "forgetme-not-owner"), true).await;
    }

    let issued_at = chrono::Utc::now().timestamp();
    let embed = serenity::CreateEmbed::new()
        .title(i18n::t(&locale, "forgetme-warning-title"))
        .description(i18n::t_args(
            &locale,
            "forgetme-warning-description",
            &[("seconds", buttons::CONFIRM_TIMEOUT_SECS.into())],
        ))
        .thumbnail(ctx.cache().current_user().face())
        .colour(util::COLOR_ERROR);

    let components = vec![serenity::CreateActionRow::Buttons(vec![
        serenity::CreateButton::new(buttons::custom_id(buttons::ForgetmeButton::Confirm, issued_at))
            .style(serenity::ButtonStyle::Danger)
            .emoji('🧹')
            .label(i18n::t(&locale, "forgetme-button-confirm")),
        serenity::CreateButton::new(buttons::custom_id(buttons::ForgetmeButton::Cancel, issued_at))
            .style(serenity::ButtonStyle::Secondary)
            .label(i18n::t(&locale, "forgetme-button-cancel")),
    ])];

    ctx.send(
        poise::CreateReply::default()
            .embed(embed)
            .components(components),
    )
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::i18n;

    #[test]
    fn catalog_has_every_forgetme_message() {
        let locale = i18n::resolve(None);
        for key in [
            "forgetme-not-owner",
            "forgetme-warning-title",
            "forgetme-button-confirm",
            "forgetme-button-cancel",
            "forgetme-cancelled-title",
            "forgetme-cancelled",
            "forgetme-expired-title",
            "forgetme-expired",
            "forgetme-goodbye-title",
            "forgetme-goodbye",
        ] {
            assert_ne!(i18n::t(&locale, key), key, "missing Fluent key {key}");
        }
    }

    #[test]
    fn warning_description_renders_the_timeout() {
        let locale = i18n::resolve(None);
        let description = i18n::t_args(
            &locale,
            "forgetme-warning-description",
            &[("seconds", crate::bot::buttons::CONFIRM_TIMEOUT_SECS.into())],
        );
        // Fluent wraps interpolated numbers in bidi isolation marks, so check
        // for the digits rather than a plain substring with spaces around it.
        assert!(description.contains("60"), "got: {description}");
    }
}
