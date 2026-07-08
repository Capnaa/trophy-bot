//! Shared trophy resolver + autocomplete (F12, born in batch C2).
//!
//! Replaces the legacy `getTrophy` (numeric-ID branch with quick.db path
//! traversal + substring name matching + empty-normalization match-all) with:
//! - [`resolve_trophy`]: EXACT normalized-name lookup (ADR 0005), guild-scoped,
//!   via a parameterized query — no traversal, no substring surprises, no
//!   lowest-ID tiebreaks (normalized names are unique per guild).
//! - [`autocomplete_trophy`]: the user-facing ergonomics of the old fuzzy
//!   matching live HERE instead — prefix match on the normalized name, at most
//!   [`MAX_CHOICES`] choices, trophy name shown AND sent as the value.
//!
//! Later batches (award, revoke, delete, edit, details) reuse both.

use sea_orm::{ColumnTrait, DbErr, EntityTrait, QueryFilter, QueryOrder, QuerySelect};

use crate::bot::{util, Context, Error};
use crate::domain::normalize::normalize_name;
use crate::entities::trophies;
use crate::i18n;

/// Discord's hard cap on autocomplete choices.
pub const MAX_CHOICES: usize = 25;

/// Resolves user input to a trophy by EXACT normalized-name match within the
/// guild (case/punctuation/whitespace-insensitive per ADR 0005). Returns
/// `Ok(None)` when nothing matches — the caller decides the error message.
pub async fn resolve_trophy(
    db: &impl sea_orm::ConnectionTrait,
    guild_id: i64,
    input: &str,
) -> Result<Option<trophies::Model>, DbErr> {
    trophies::Entity::find()
        .filter(trophies::Column::GuildId.eq(guild_id))
        .filter(trophies::Column::NormalizedName.eq(normalize_name(input)))
        .one(db)
        .await
}

/// [`resolve_trophy`] plus the shared not-found handling: when nothing
/// matches, replies with the caller's localized `<not_found_key>` message
/// (an ephemeral error carrying the raw input as `$input`) and returns
/// `Ok(None)` — the caller just returns `Ok(())`. Keeps the six identical
/// resolve-or-error blocks (/award /revoke /delete /edit /details /show) in
/// one place.
pub async fn resolve_trophy_or_reply(
    ctx: Context<'_>,
    guild_id: i64,
    input: &str,
    not_found_key: &str,
) -> Result<Option<trophies::Model>, Error> {
    let model = resolve_trophy(&ctx.data().db, guild_id, input).await?;
    if model.is_none() {
        let locale = util::locale(&ctx);
        util::reply_error(
            ctx,
            i18n::t_args(&locale, not_found_key, &[("input", input.to_string().into())]),
            true,
        )
        .await?;
    }
    Ok(model)
}

/// Pure choice filter behind [`autocomplete_trophy`]: keeps the trophies whose
/// normalized name starts with the normalized partial input, preserving the
/// caller's ordering, capped at [`MAX_CHOICES`]. An empty partial normalizes
/// to `""` so every trophy matches — the user sees the guild's trophies right
/// away when the option field is still blank.
pub fn prefix_choices(names: &[(String, String)], partial: &str) -> Vec<String> {
    let needle = normalize_name(partial);
    // `normalize_name("")` falls back to the lowercased raw input, so a
    // whitespace-only partial yields e.g. " " which would match nothing;
    // treat blank input as "show everything" explicitly.
    let needle = if partial.trim().is_empty() { String::new() } else { needle };
    names
        .iter()
        .filter(|(_, normalized)| normalized.starts_with(&needle))
        .map(|(name, _)| name.clone())
        .take(MAX_CHOICES)
        .collect()
}

/// Poise autocomplete callback for `trophy` options: prefix match on the
/// guild's normalized trophy names, alphabetical, max [`MAX_CHOICES`]. The
/// choice label and value are both the trophy name (names are unique per
/// guild). Autocomplete cannot report errors to the user, so DB failures are
/// logged and yield no choices.
pub async fn autocomplete_trophy(ctx: Context<'_>, partial: &str) -> Vec<String> {
    let Some(guild_id) = ctx.guild_id() else {
        return Vec::new();
    };
    let names: Result<Vec<(String, String)>, DbErr> = trophies::Entity::find()
        .filter(trophies::Column::GuildId.eq(guild_id.get() as i64))
        .select_only()
        .column(trophies::Column::Name)
        .column(trophies::Column::NormalizedName)
        .order_by_asc(trophies::Column::Name)
        .into_tuple()
        .all(&ctx.data().db)
        .await;
    match names {
        Ok(names) => prefix_choices(&names, partial),
        Err(err) => {
            log::warn!(
                "trophy autocomplete query failed (guild={}): {err}",
                guild_id.get()
            );
            Vec::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sea_orm::{ActiveModelTrait, DatabaseConnection, Set};
    use uuid::Uuid;

    use crate::domain::test_support::{fresh_db, insert_guild, now};
    use crate::entities::guilds;

    /// Inserts a trophy, auto-creating the guild row the FK needs.
    async fn insert_trophy(db: &DatabaseConnection, guild_id: i64, name: &str) -> Uuid {
        if guilds::Entity::find_by_id(guild_id).one(db).await.unwrap().is_none() {
            insert_guild(db, guild_id).await;
        }
        let id = Uuid::now_v7();
        trophies::ActiveModel {
            id: Set(id),
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
            details: Set("No details provided.".to_string()),
            signed: Set(false),
            created_at: Set(now()),
            updated_at: Set(now()),
        }
        .insert(db)
        .await
        .expect("insert trophy");
        id
    }

    // --- resolve_trophy ---

    #[tokio::test]
    async fn resolves_exact_name_case_and_punctuation_insensitive() {
        let db = fresh_db().await;
        let id = insert_trophy(&db, 1, "Gold Medal").await;

        for input in ["Gold Medal", "gold medal", "GOLD-MEDAL!", "  g o l d medal  "] {
            let found = resolve_trophy(&db, 1, input).await.unwrap();
            assert_eq!(found.map(|t| t.id), Some(id), "input: {input:?}");
        }
    }

    #[tokio::test]
    async fn substring_input_does_not_resolve() {
        // F12: legacy substring matching ("gold" found "Golden Medal") is gone.
        let db = fresh_db().await;
        insert_trophy(&db, 1, "Golden Medal").await;

        assert!(resolve_trophy(&db, 1, "gold").await.unwrap().is_none());
        assert!(resolve_trophy(&db, 1, "golden").await.unwrap().is_none());
        assert!(resolve_trophy(&db, 1, "Golden Medal").await.unwrap().is_some());
    }

    #[tokio::test]
    async fn punctuation_only_input_matches_nothing_not_everything() {
        // F12: legacy empty-normalization input matched EVERY trophy.
        let db = fresh_db().await;
        insert_trophy(&db, 1, "Gold Medal").await;
        insert_trophy(&db, 1, "Silver Medal").await;

        assert!(resolve_trophy(&db, 1, "!!!").await.unwrap().is_none());
        assert!(resolve_trophy(&db, 1, "").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn emoji_named_trophies_resolve_exactly() {
        let db = fresh_db().await;
        let cup = insert_trophy(&db, 1, "🏆").await;
        insert_trophy(&db, 1, "Gold Medal").await;

        let found = resolve_trophy(&db, 1, "🏆").await.unwrap();
        assert_eq!(found.map(|t| t.id), Some(cup));
        // A different emoji matches nothing (not the lowest-ID trophy).
        assert!(resolve_trophy(&db, 1, "🥇").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn dotted_input_cannot_traverse() {
        // F12: legacy `1.name` "resolved" through quick.db path traversal.
        let db = fresh_db().await;
        insert_trophy(&db, 1, "Gold Medal").await;

        assert!(resolve_trophy(&db, 1, "1.name").await.unwrap().is_none());
        // Unless a trophy is literally named that way (normalizes to "1name").
        let dotted = insert_trophy(&db, 1, "1 Name").await;
        let found = resolve_trophy(&db, 1, "1.name").await.unwrap();
        assert_eq!(found.map(|t| t.id), Some(dotted));
    }

    #[tokio::test]
    async fn resolution_is_guild_scoped() {
        let db = fresh_db().await;
        let here = insert_trophy(&db, 1, "Gold Medal").await;
        let there = insert_trophy(&db, 2, "Gold Medal").await;

        assert_eq!(resolve_trophy(&db, 1, "gold medal").await.unwrap().map(|t| t.id), Some(here));
        assert_eq!(resolve_trophy(&db, 2, "gold medal").await.unwrap().map(|t| t.id), Some(there));
        assert!(resolve_trophy(&db, 3, "gold medal").await.unwrap().is_none());
    }

    // --- prefix_choices ---

    fn pairs(names: &[&str]) -> Vec<(String, String)> {
        names.iter().map(|n| (n.to_string(), normalize_name(n))).collect()
    }

    #[test]
    fn prefix_matches_normalized_names() {
        let names = pairs(&["Golden Medal", "Gold Star", "Silver Medal"]);
        assert_eq!(prefix_choices(&names, "gold"), vec!["Golden Medal", "Gold Star"]);
        assert_eq!(prefix_choices(&names, "GOLD-S"), vec!["Gold Star"]);
        assert_eq!(prefix_choices(&names, "silver medal"), vec!["Silver Medal"]);
        assert_eq!(prefix_choices(&names, "medal"), Vec::<String>::new());
    }

    #[test]
    fn blank_partial_lists_everything() {
        let names = pairs(&["Alpha", "Beta", "🏆"]);
        assert_eq!(prefix_choices(&names, ""), vec!["Alpha", "Beta", "🏆"]);
        assert_eq!(prefix_choices(&names, "   "), vec!["Alpha", "Beta", "🏆"]);
    }

    #[test]
    fn emoji_partial_matches_only_emoji_named_trophies() {
        let names = pairs(&["Gold Medal", "🏆"]);
        assert_eq!(prefix_choices(&names, "🏆"), vec!["🏆"]);
        // Punctuation-only input normalizes to itself and matches nothing.
        assert_eq!(prefix_choices(&names, "!!!"), Vec::<String>::new());
    }

    #[test]
    fn choices_are_capped_at_discord_limit() {
        let all: Vec<String> = (0..40).map(|i| format!("Trophy {i:02}")).collect();
        let names = pairs(&all.iter().map(String::as_str).collect::<Vec<_>>());
        let choices = prefix_choices(&names, "trophy");
        assert_eq!(choices.len(), MAX_CHOICES);
        assert_eq!(choices[0], "Trophy 00");
    }

    #[test]
    fn choices_preserve_input_order() {
        // The DB query orders by name; the filter must not reorder.
        let names = pairs(&["Gold B", "Gold A"]);
        assert_eq!(prefix_choices(&names, "gold"), vec!["Gold B", "Gold A"]);
    }
}
