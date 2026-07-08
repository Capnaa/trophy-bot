//! Shared helpers for command implementations: embed colors, locale
//! resolution, reply shortcuts, guild-id extraction and pagination. Every
//! command batch builds on these.

use std::sync::atomic::Ordering;

use poise::serenity_prelude as serenity;

use crate::bot::{Context, Error};
use crate::i18n::{self, LanguageIdentifier};

/// Main brand color (`#0096FF`, legacy `color.main`).
pub const COLOR_MAIN: u32 = 0x0096FF;
/// Error color (`#E02D44`, legacy `color.error`).
pub const COLOR_ERROR: u32 = 0xE02D44;
/// Success color (award/create/edit/delete/etc. confirmations).
pub const COLOR_SUCCESS: u32 = 0x2ECC71;

/// Resolves the Fluent locale for this interaction (exact tag → language
/// prefix → `en-US`). The ctx-locale shortcut every command uses.
pub fn locale(ctx: &Context<'_>) -> LanguageIdentifier {
    i18n::resolve(ctx.locale())
}

/// The invoking guild id. Every command that calls this is declared
/// `guild_only`, so absence is an internal error (poise rejects DM
/// invocations before the handler runs), not a user-facing one.
pub fn require_guild_id(ctx: &Context<'_>) -> Result<serenity::GuildId, Error> {
    ctx.guild_id()
        .ok_or_else(|| anyhow::anyhow!("guild_only command invoked outside a guild"))
}

/// Legacy `getPage` semantics (shared by every paginated list):
/// `last = ceil(len / per_page)` floored at 1, `page` clamped to `[1, last]`;
/// returns the slice, the clamped page and `last`. An empty list yields
/// `([], 1, 1)`.
pub fn paginate<T>(items: &[T], per_page: usize, requested: i64) -> (&[T], usize, usize) {
    let last = items.len().div_ceil(per_page).max(1);
    let page = requested.clamp(1, last as i64) as usize;
    let start = (page - 1) * per_page;
    let end = usize::min(start + per_page, items.len());
    (&items[start..end], page, last)
}

/// Sends (or follows up with) a single-embed reply. `ephemeral` must be
/// chosen explicitly by the caller — no hidden default.
pub async fn reply_embed(
    ctx: Context<'_>,
    embed: serenity::CreateEmbed,
    ephemeral: bool,
) -> Result<(), Error> {
    ctx.send(poise::CreateReply::default().embed(embed).ephemeral(ephemeral))
        .await?;
    Ok(())
}

/// Sends a localized-description error embed (error color + localized title).
/// `ephemeral` must be chosen explicitly; error replies are ephemeral unless
/// a spec says otherwise.
pub async fn reply_error(
    ctx: Context<'_>,
    description: impl Into<String>,
    ephemeral: bool,
) -> Result<(), Error> {
    let locale = locale(&ctx);
    let embed = serenity::CreateEmbed::new()
        .title(i18n::t(&locale, "common-error-title"))
        .description(description.into())
        .colour(COLOR_ERROR);
    reply_embed(ctx, embed, ephemeral).await
}

/// Error reply that stays EPHEMERAL even when the interaction was already
/// publicly deferred (rust-parity-plan §2: all error replies are ephemeral).
///
/// Discord locks a deferred response's visibility at defer time: once a
/// public `ctx.defer()` went out, a followup's `ephemeral(true)` flag cannot
/// make the already-visible "thinking…" placeholder private. So when an
/// initial response exists, it is deleted first (best effort — deleting an
/// EPHEMERAL deferred response is not always possible, in which case the
/// followup hydrates that private placeholder anyway) and the error goes out
/// as a fresh ephemeral followup. Without a prior response this is a plain
/// ephemeral reply.
pub async fn reply_error_ephemeral(
    ctx: Context<'_>,
    description: impl Into<String>,
) -> Result<(), Error> {
    if let poise::Context::Application(app) = ctx
        && app.has_sent_initial_response.load(Ordering::SeqCst)
        && let Err(err) = app.interaction.delete_response(ctx.http()).await
    {
        log::debug!(
            "could not delete the deferred response before an ephemeral error reply: {err}"
        );
    }
    reply_error(ctx, description, true).await
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The embed colors must keep matching the legacy Node.js palette
    /// (`color.main` / `color.error` in globals.js) plus the success green.
    #[test]
    fn embed_colors_match_legacy_palette() {
        assert_eq!(COLOR_MAIN, 0x0096FF);
        assert_eq!(COLOR_ERROR, 0xE02D44);
        assert_eq!(COLOR_SUCCESS, 0x2ECC71);
    }

    // --- paginate (shared legacy getPage semantics) ---

    #[test]
    fn paginate_empty_list_is_single_empty_page() {
        let items: [i32; 0] = [];
        let (slice, page, last) = paginate(&items, 10, 1);
        assert!(slice.is_empty());
        assert_eq!((page, last), (1, 1));
    }

    #[test]
    fn paginate_clamps_out_of_range_pages() {
        let items: Vec<i32> = (0..25).collect();
        let (slice, page, last) = paginate(&items, 10, -5);
        assert_eq!((slice.len(), page, last), (10, 1, 3));
        let (slice, page, last) = paginate(&items, 10, 99);
        assert_eq!((slice.len(), page, last), (5, 3, 3));
    }

    #[test]
    fn paginate_slices_the_requested_page() {
        let items: Vec<i32> = (0..25).collect();
        let (slice, page, last) = paginate(&items, 10, 2);
        assert_eq!(slice, &items[10..20]);
        assert_eq!((page, last), (2, 3));
    }

    #[test]
    fn paginate_supports_non_default_page_sizes() {
        // /rewards list uses 5 per page.
        let items: Vec<i32> = (0..12).collect();
        let (slice, page, last) = paginate(&items, 5, 3);
        assert_eq!(slice, &items[10..12]);
        assert_eq!((page, last), (3, 3));
    }

    /// The guild-only boilerplate must live HERE, not be copy-pasted per
    /// command: every command module goes through [`require_guild_id`].
    #[test]
    fn no_command_module_duplicates_the_guild_id_boilerplate() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/src/bot/commands");
        for entry in std::fs::read_dir(dir).expect("read commands dir") {
            let path = entry.expect("dir entry").path();
            if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                continue;
            }
            let source = std::fs::read_to_string(&path).expect("read command source");
            assert!(
                !source.contains("invoked outside a guild")
                    && !source.contains("invoked without a guild"),
                "{} re-implements the guild-id boilerplate; use util::require_guild_id",
                path.display()
            );
        }
    }
}
