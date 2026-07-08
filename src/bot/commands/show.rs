//! `/show` — publicly display a trophy (batch C2).
//!
//! Spec: docs/specs/commands-user.md §/show. Fixes applied:
//! - F12: trophy resolved by exact normalized name with autocomplete
//!   (`src/bot/resolver.rs`) — no numeric-ID branch, no substring matching,
//!   no path traversal, no "Trophy ID" footer (IDs are never user-facing).
//! - F17: a stored local image whose file is missing falls back to the
//!   default trophy image with a logged warning — the reply never hangs and
//!   the process never crashes.
//! - F36: dedication display mode 1 ("Always Name") resolves the LIVE
//!   display name (guild nick → global name → username), falling back to the
//!   stored `dedication_text`, then to a mention.
//!
//! Business logic lives in plain testable functions; the handler stays thin.

use std::path::Path;

use poise::serenity_prelude as serenity;

use crate::bot::{Context, Error, images, resolver, util};
use crate::domain::settings::{self, Setting};
use crate::i18n;

/// Default trophy image, used when a trophy has no image or its local file
/// is gone (legacy `show.js:33`).
pub(crate) const DEFAULT_IMAGE_URL: &str =
    "https://cdn.discordapp.com/attachments/631540341148876802/985219082662064178/trophy.png";

/// Legacy easter-egg embed URL, kept deliberately for parity (`show.js:40`).
const EMBED_URL: &str = "https://www.youtube.com/watch?v=04854XqcfCY";

/// How the trophy image will be rendered on the embed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ImagePlan {
    /// Direct URL on the embed (stored https URL or the default image).
    Url(String),
    /// Local file uploaded as an attachment, embed points at `attachment://`.
    Attachment { filename: String, bytes: Vec<u8> },
}

/// Decides the image source for a trophy (F17):
/// - no stored image → default image URL;
/// - stored `https://` URL → passthrough (legacy data holds some CDN URLs);
/// - anything else is a filename under `images_dir` — read it, and on ANY
///   failure (missing file, bogus name with path separators) log a warning
///   and fall back to the default image instead of failing the reply.
pub(crate) async fn plan_image(images_dir: &Path, stored: Option<&str>) -> ImagePlan {
    let Some(stored) = stored else {
        return ImagePlan::Url(DEFAULT_IMAGE_URL.to_string());
    };
    if stored.starts_with("https://") {
        return ImagePlan::Url(stored.to_string());
    }
    if stored.contains('/') || stored.contains('\\') {
        log::warn!("trophy image filename contains a path separator, using default: {stored:?}");
        return ImagePlan::Url(DEFAULT_IMAGE_URL.to_string());
    }
    match tokio::fs::read(images_dir.join(stored)).await {
        Ok(bytes) => ImagePlan::Attachment { filename: stored.to_string(), bytes },
        Err(err) => {
            log::warn!("trophy image file {stored:?} unreadable, using default: {err}");
            ImagePlan::Url(DEFAULT_IMAGE_URL.to_string())
        }
    }
}

/// Formats the "Dedicated to" line per the `dedication_display` setting.
/// Returns `None` when the trophy has no dedication at all.
///
/// - text-only dedication → the stored text, whatever the mode;
/// - mode 0 "Always Mention" → `<@id>` (no fetch needed);
/// - mode 1 "Always Name" (F36) → live display name → stored text → mention;
/// - mode 2 "Mention Only in Server" (default) → mention if the user is in
///   the server, else stored text (mention as last resort when no text was
///   stored — legacy data always has one, post-cutover rows may not).
pub(crate) fn format_dedication(
    mode: i16,
    user_id: Option<i64>,
    stored_text: Option<&str>,
    live_name: Option<&str>,
    in_server: bool,
) -> Option<String> {
    let Some(id) = user_id else {
        return stored_text.map(str::to_string);
    };
    let mention = || format!("<@{id}>");
    Some(match mode {
        0 => mention(),
        1 => live_name
            .or(stored_text)
            .map_or_else(mention, str::to_string),
        _ => {
            if in_server {
                mention()
            } else {
                stored_text.map_or_else(mention, str::to_string)
            }
        }
    })
}

/// Best-effort live lookup of the dedication user: guild member first (gives
/// the in-server flag and the nick-aware display name), then — only when a
/// name is actually needed (mode 1) — a global user fetch for people who
/// left. Every failure is swallowed: display falls back gracefully (F36).
async fn live_member_info(
    ctx: &Context<'_>,
    guild_id: serenity::GuildId,
    user_id: i64,
    want_name: bool,
) -> (Option<String>, bool) {
    let uid = serenity::UserId::new(user_id as u64);
    match guild_id.member(ctx.serenity_context(), uid).await {
        Ok(member) => (Some(member.display_name().to_string()), true),
        Err(err) => {
            log::debug!("dedication member {user_id} not resolved in guild {guild_id}: {err}");
            if !want_name {
                return (None, false);
            }
            match uid.to_user(ctx.serenity_context()).await {
                Ok(user) => {
                    let name = user.global_name.clone().unwrap_or_else(|| user.name.clone());
                    (Some(name), false)
                }
                Err(err) => {
                    log::debug!("dedication user {user_id} not resolved globally: {err}");
                    (None, false)
                }
            }
        }
    }
}

/// Show a trophy
#[poise::command(slash_command, guild_only)]
pub async fn show(
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
            i18n::t_args(&locale, "show-error-not-found", &[("input", trophy.into())]),
            true,
        )
        .await;
    };

    // Dedication line: fetch live info only when the display mode needs it.
    let mode = settings::get_setting(db, guild_id.get() as i64, Setting::DedicationDisplay).await?;
    let (live_name, in_server) = match model.dedication_user_id {
        Some(user_id) if mode != 0 => {
            live_member_info(&ctx, guild_id, user_id, mode == 1).await
        }
        _ => (None, false),
    };
    let dedication = format_dedication(
        mode,
        model.dedication_user_id,
        model.dedication_text.as_deref(),
        live_name.as_deref(),
        in_server,
    );

    let image = plan_image(Path::new(images::IMAGES_DIR), model.image.as_deref()).await;

    let mut embed = serenity::CreateEmbed::new()
        .title(format!("{} {}", model.emoji, model.name))
        .url(EMBED_URL)
        .description(model.description.clone())
        .colour(util::COLOR_MAIN)
        .field(
            i18n::t(&locale, "show-field-value"),
            i18n::t_args(&locale, "show-value", &[("value", i64::from(model.value).into())]),
            true,
        )
        .footer(serenity::CreateEmbedFooter::new(i18n::t_args(
            &locale,
            "show-footer",
            &[("name", model.name.clone().into())],
        )));

    if model.signed {
        if let Some(creator) = model.creator_user_id {
            embed = embed.field(
                i18n::t(&locale, "show-field-signed"),
                format!("<@{creator}>"),
                true,
            );
        }
    }
    if let Some(dedicated_to) = dedication {
        embed = embed.field(i18n::t(&locale, "show-field-dedicated"), dedicated_to, true);
    }

    let mut reply = poise::CreateReply::default().ephemeral(false);
    match image {
        ImagePlan::Url(url) => embed = embed.image(url),
        ImagePlan::Attachment { filename, bytes } => {
            embed = embed.attachment(filename.clone());
            reply = reply.attachment(serenity::CreateAttachment::bytes(bytes, filename));
        }
    }
    ctx.send(reply.embed(embed)).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- format_dedication ---

    #[test]
    fn no_dedication_renders_nothing() {
        for mode in 0..=2 {
            assert_eq!(format_dedication(mode, None, None, None, false), None);
            assert_eq!(format_dedication(mode, None, None, Some("live"), true), None);
        }
    }

    #[test]
    fn text_only_dedication_shows_stored_text_in_every_mode() {
        for mode in 0..=2 {
            assert_eq!(
                format_dedication(mode, None, Some("mom"), None, false),
                Some("mom".to_string()),
                "mode {mode}"
            );
        }
    }

    #[test]
    fn mode_0_always_mentions() {
        assert_eq!(
            format_dedication(0, Some(42), Some("stored"), Some("live"), false),
            Some("<@42>".to_string())
        );
    }

    #[test]
    fn mode_1_prefers_live_name_then_stored_text_then_mention() {
        // F36: the live display name must win when available.
        assert_eq!(
            format_dedication(1, Some(42), Some("stored"), Some("Live Nick"), true),
            Some("Live Nick".to_string())
        );
        // Live name also comes from a global fetch for departed users.
        assert_eq!(
            format_dedication(1, Some(42), Some("stored"), Some("Global"), false),
            Some("Global".to_string())
        );
        assert_eq!(
            format_dedication(1, Some(42), Some("stored"), None, false),
            Some("stored".to_string())
        );
        assert_eq!(
            format_dedication(1, Some(42), None, None, false),
            Some("<@42>".to_string())
        );
    }

    #[test]
    fn mode_2_mentions_members_and_names_absentees() {
        assert_eq!(
            format_dedication(2, Some(42), Some("stored"), None, true),
            Some("<@42>".to_string())
        );
        assert_eq!(
            format_dedication(2, Some(42), Some("stored"), None, false),
            Some("stored".to_string())
        );
        // No stored text (possible post-cutover): mention as last resort.
        assert_eq!(
            format_dedication(2, Some(42), None, None, false),
            Some("<@42>".to_string())
        );
    }

    // --- plan_image (F17) ---

    /// Unique throwaway directory for image tests.
    fn temp_dir() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("trophy-bot-show-test-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    #[tokio::test]
    async fn no_image_uses_the_default_url() {
        let dir = temp_dir();
        assert_eq!(plan_image(&dir, None).await, ImagePlan::Url(DEFAULT_IMAGE_URL.to_string()));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn https_image_is_passed_through_without_touching_disk() {
        let dir = temp_dir();
        let url = "https://cdn.discordapp.com/attachments/1/2/pic.png";
        assert_eq!(plan_image(&dir, Some(url)).await, ImagePlan::Url(url.to_string()));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn existing_local_file_is_attached_with_its_bytes() {
        let dir = temp_dir();
        std::fs::write(dir.join("1_abc.png"), b"png-bytes").expect("write image");
        assert_eq!(
            plan_image(&dir, Some("1_abc.png")).await,
            ImagePlan::Attachment { filename: "1_abc.png".to_string(), bytes: b"png-bytes".to_vec() }
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn missing_local_file_falls_back_to_the_default() {
        // F17: this legacy case hung the reply and could crash the process.
        let dir = temp_dir();
        assert_eq!(
            plan_image(&dir, Some("gone.png")).await,
            ImagePlan::Url(DEFAULT_IMAGE_URL.to_string())
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn path_separators_in_the_filename_fall_back_to_the_default() {
        let dir = temp_dir();
        for stored in ["../secret.png", "a/b.png", "..\\evil.png"] {
            assert_eq!(
                plan_image(&dir, Some(stored)).await,
                ImagePlan::Url(DEFAULT_IMAGE_URL.to_string()),
                "stored: {stored:?}"
            );
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn non_https_urls_are_treated_as_filenames_and_fall_back() {
        // Legacy checked only the `https://` prefix; an `http://` value is a
        // (nonexistent) filename with separators → default image.
        let dir = temp_dir();
        assert_eq!(
            plan_image(&dir, Some("http://example.com/pic.png")).await,
            ImagePlan::Url(DEFAULT_IMAGE_URL.to_string())
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    // --- i18n catalog ---

    #[test]
    fn all_show_messages_exist() {
        let locale = i18n::resolve(None);
        let args: &[(&'static str, i18n::FluentValue<'static>)] =
            &[("input", "Gold".into()), ("value", 10.into()), ("name", "Gold".into())];
        for key in [
            "show-error-not-found",
            "show-field-value",
            "show-value",
            "show-field-signed",
            "show-field-dedicated",
            "show-footer",
        ] {
            assert_ne!(i18n::t_args(&locale, key, args), key, "missing ftl message: {key}");
        }
    }

    #[test]
    fn not_found_message_renders_the_input() {
        let locale = i18n::resolve(None);
        let message =
            i18n::t_args(&locale, "show-error-not-found", &[("input", "Golde".into())]);
        assert!(message.contains("Golde"), "got: {message}");
    }
}
