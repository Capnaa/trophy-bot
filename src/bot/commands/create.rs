//! `/create` — create a new trophy (batch C1).
//!
//! Spec: docs/specs/commands-trophy-management.md §/create. Parity fixes:
//! - F3: EVERYTHING (text limits, value range, capacity, name uniqueness,
//!   image content-type + 1 MB) is validated before any persistence; the
//!   image is downloaded first and the row is inserted in one transaction.
//! - F4: `value` is an integer option, range ±999,999 enforced server-side.
//! - F5: normalized-name uniqueness per guild (ADR 0005) with a clear error.
//!
//! Business logic lives in plain testable functions; the poise handler at the
//! bottom stays thin.

use poise::serenity_prelude as serenity;
use sea_orm::sea_query::OnConflict;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DatabaseConnection, EntityTrait,
    PaginatorTrait, QueryFilter, Set, TransactionTrait,
};
use uuid::Uuid;

use crate::bot::{images, util, Context, Error};
use crate::domain::normalize::normalize_name;
use crate::entities::{guilds, trophies};
use crate::i18n::{self, LanguageIdentifier};

/// Per-guild trophy cap. Kept at the legacy 150 for parity at cutover
/// (rust-parity-plan §4.5) — a config value now, not a technical ceiling.
pub(crate) const MAX_TROPHIES_PER_GUILD: u64 = 150;

// Legacy field limits (spec §/create "Validation rules & limits").
const MAX_NAME_CHARS: usize = 32;
const MAX_DESCRIPTION_CHARS: usize = 128;
const MAX_EMOJI_CHARS: usize = 64;
const MAX_DEDICATION_CHARS: usize = 32;
const MAX_DETAILS_CHARS: usize = 300;
const MAX_VALUE: i32 = 999_999;

// Stored defaults (schema.md; language-independent, kept from legacy).
const DEFAULT_DESCRIPTION: &str = "No description provided";
const DEFAULT_EMOJI: &str = "🏆";
const DEFAULT_DETAILS: &str = "No details provided.";
const DEFAULT_VALUE: i32 = 10;

/// Everything that can make `/create` refuse, mapped 1:1 to localized
/// messages in `locales/en-US/create.ftl`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CreateError {
    /// Name is empty or whitespace-only (would normalize to an empty
    /// `normalized_name` and break guild-wide trophy autocomplete).
    EmptyName,
    /// `field` is the ftl key fragment: name/details/description/emoji/dedication.
    FieldTooLong { field: &'static str, max: usize },
    ValueOutOfRange,
    GuildFull,
    DuplicateName { name: String },
}

impl CreateError {
    /// Localized user-facing message for this rejection.
    pub(crate) fn message(&self, locale: &LanguageIdentifier) -> String {
        match self {
            Self::EmptyName => i18n::t(locale, "create-error-name-empty"),
            Self::FieldTooLong { field, max } => i18n::t_args(
                locale,
                &format!("create-error-{field}-too-long"),
                &[("max", (*max as i64).into())],
            ),
            Self::ValueOutOfRange => i18n::t_args(
                locale,
                "create-error-value-out-of-range",
                &[("min", i64::from(-MAX_VALUE).into()), ("max", i64::from(MAX_VALUE).into())],
            ),
            Self::GuildFull => i18n::t_args(
                locale,
                "create-error-guild-full",
                &[("max", (MAX_TROPHIES_PER_GUILD as i64).into())],
            ),
            Self::DuplicateName { name } => i18n::t_args(
                locale,
                "create-error-duplicate-name",
                &[("name", name.clone().into())],
            ),
        }
    }
}

/// Validates all text fields and the value, in the legacy order (spec step 2):
/// name → details → description → emoji → value → dedication. Char-counted
/// (not bytes), matching the legacy JS `.length` intent for user text.
pub(crate) fn validate_fields(
    name: &str,
    description: Option<&str>,
    emoji: Option<&str>,
    value: i32,
    dedication: Option<&str>,
    details: Option<&str>,
) -> Result<(), CreateError> {
    let too_long = |text: Option<&str>, max: usize| text.is_some_and(|t| t.chars().count() > max);

    // A blank name would store an empty normalized_name and emit an empty
    // autocomplete choice, which Discord rejects (400) — breaking autocomplete
    // for the whole guild. Discord options carry no min_length, so enforce here.
    if name.trim().is_empty() {
        return Err(CreateError::EmptyName);
    }
    if too_long(Some(name), MAX_NAME_CHARS) {
        return Err(CreateError::FieldTooLong { field: "name", max: MAX_NAME_CHARS });
    }
    if too_long(details, MAX_DETAILS_CHARS) {
        return Err(CreateError::FieldTooLong { field: "details", max: MAX_DETAILS_CHARS });
    }
    if too_long(description, MAX_DESCRIPTION_CHARS) {
        return Err(CreateError::FieldTooLong {
            field: "description",
            max: MAX_DESCRIPTION_CHARS,
        });
    }
    if too_long(emoji, MAX_EMOJI_CHARS) {
        return Err(CreateError::FieldTooLong { field: "emoji", max: MAX_EMOJI_CHARS });
    }
    if !(-MAX_VALUE..=MAX_VALUE).contains(&value) {
        return Err(CreateError::ValueOutOfRange);
    }
    if too_long(dedication, MAX_DEDICATION_CHARS) {
        return Err(CreateError::FieldTooLong {
            field: "dedication",
            max: MAX_DEDICATION_CHARS,
        });
    }
    Ok(())
}

/// Parsed dedication: a mention/raw snowflake becomes a user dedication,
/// anything else is plain text (spec: the legacy member prefix-search branch
/// was dead in this flow and is intentionally not reimplemented).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Dedication {
    User(u64),
    Text(String),
}

/// Parses the raw `dedication` option: `<@id>` / `<@!id>` mentions and bare
/// snowflakes (15–20 ASCII digits) resolve to [`Dedication::User`]; everything
/// else is kept verbatim as [`Dedication::Text`].
pub(crate) fn parse_dedication(raw: &str) -> Dedication {
    let trimmed = raw.trim();
    let candidate = trimmed
        .strip_prefix("<@")
        .and_then(|s| s.strip_suffix('>'))
        .map(|s| s.strip_prefix('!').unwrap_or(s))
        .unwrap_or(trimmed);

    let looks_like_snowflake = (15..=20).contains(&candidate.len())
        && candidate.bytes().all(|b| b.is_ascii_digit());
    if looks_like_snowflake
        && let Ok(id) = candidate.parse::<u64>()
        && id > 0
    {
        return Dedication::User(id);
    }
    Dedication::Text(raw.to_string())
}

/// Fully-validated trophy, ready to insert. The UUID is caller-generated so
/// the image filename can be derived before the row exists.
#[derive(Debug, Clone)]
pub(crate) struct NewTrophy {
    pub id: Uuid,
    pub guild_id: i64,
    pub creator_user_id: i64,
    pub name: String,
    pub description: String,
    pub emoji: String,
    pub value: i32,
    pub image: Option<String>,
    pub dedication_user_id: Option<i64>,
    pub dedication_text: Option<String>,
    pub details: String,
    pub signed: bool,
}

/// DB prechecks that must pass before the image download / insert: guild
/// capacity first (legacy checked it before anything else), then the
/// normalized-name uniqueness (F5). `Ok(Some(_))` is a user-facing rejection;
/// `Err` is infrastructure failure.
pub(crate) async fn precheck(
    db: &impl ConnectionTrait,
    guild_id: i64,
    name: &str,
) -> anyhow::Result<Option<CreateError>> {
    let count = trophies::Entity::find()
        .filter(trophies::Column::GuildId.eq(guild_id))
        .count(db)
        .await?;
    if count >= MAX_TROPHIES_PER_GUILD {
        return Ok(Some(CreateError::GuildFull));
    }

    let duplicate = trophies::Entity::find()
        .filter(trophies::Column::GuildId.eq(guild_id))
        .filter(trophies::Column::NormalizedName.eq(normalize_name(name)))
        .count(db)
        .await?;
    if duplicate > 0 {
        return Ok(Some(CreateError::DuplicateName { name: name.to_string() }));
    }
    Ok(None)
}

/// Inserts the trophy in one transaction (F3), auto-registering the guild row
/// (FK) without clobbering an existing one. The inner `Err` is the
/// race-window duplicate (unique index on `(guild_id, normalized_name)`
/// fired between [`precheck`] and the insert).
pub(crate) async fn insert_trophy(
    db: &DatabaseConnection,
    new: NewTrophy,
) -> anyhow::Result<Result<trophies::Model, CreateError>> {
    let now = chrono::Utc::now().naive_utc();
    let txn = db.begin().await?;

    guilds::Entity::insert(guilds::ActiveModel {
        id: Set(new.guild_id),
        is_safe: Set(false),
        created_at: Set(now),
        updated_at: Set(now),
    })
    .on_conflict(OnConflict::column(guilds::Column::Id).do_nothing().to_owned())
    .exec_without_returning(&txn)
    .await?;

    let name = new.name.clone();
    let inserted = trophies::ActiveModel {
        id: Set(new.id),
        guild_id: Set(new.guild_id),
        legacy_id: Set(None),
        creator_user_id: Set(Some(new.creator_user_id)),
        normalized_name: Set(normalize_name(&new.name)),
        name: Set(new.name),
        description: Set(new.description),
        emoji: Set(new.emoji),
        value: Set(new.value),
        image: Set(new.image),
        dedication_user_id: Set(new.dedication_user_id),
        dedication_text: Set(new.dedication_text),
        details: Set(new.details),
        signed: Set(new.signed),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(&txn)
    .await;

    match inserted {
        Ok(model) => {
            txn.commit().await?;
            Ok(Ok(model))
        }
        Err(err)
            if matches!(err.sql_err(), Some(sea_orm::SqlErr::UniqueConstraintViolation(_))) =>
        {
            txn.rollback().await.ok();
            Ok(Err(CreateError::DuplicateName { name }))
        }
        Err(err) => {
            txn.rollback().await.ok();
            Err(err.into())
        }
    }
}

/// Maps a parsed dedication plus the (attempted) user fetch onto the stored
/// `(dedication_user_id, dedication_text)` columns. Pure so the fetch-failure
/// path is testable; shared with `/edit`.
///
/// Legacy parity (globals.js `parseUser`: fetch failure → `notfound` → the
/// raw text was stored): a mention/snowflake that does NOT resolve to a real
/// user becomes a TEXT dedication holding the raw input — never a user
/// dedication pointing at a broken `<@…>` mention with no text to fall back
/// to.
pub(crate) fn dedication_columns(
    raw: &str,
    parsed: Dedication,
    fetched_name: Option<String>,
) -> (Option<i64>, Option<String>) {
    match parsed {
        Dedication::Text(text) => (None, Some(text)),
        Dedication::User(id) => match fetched_name {
            Some(name) => (Some(id as i64), Some(name)),
            None => (None, Some(raw.to_string())),
        },
    }
}

/// Resolves the dedication option into the stored columns. For a user
/// dedication the display-name snapshot (`dedication_text`) is fetched —
/// legacy stored the username at creation time so display mode 1 still works
/// after the user leaves (F36 consumer); an unresolvable user falls back to
/// a text dedication (see [`dedication_columns`]).
async fn resolve_dedication(
    ctx: &Context<'_>,
    raw: Option<&str>,
) -> (Option<i64>, Option<String>) {
    let Some(raw) = raw else {
        return (None, None);
    };
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

/// Create a new trophy for your server.
#[allow(clippy::too_many_arguments)]
#[poise::command(slash_command, guild_only, default_member_permissions = "MANAGE_GUILD", required_permissions = "MANAGE_GUILD")]
pub async fn create(
    ctx: Context<'_>,
    #[description = "The name of the trophy."]
    #[min_length = 1]
    #[max_length = 32]
    name: String,
    #[description = "Description for the trophy"] description: Option<String>,
    #[description = "An emoji for the trophy, leave blank for default"] emoji: Option<String>,
    #[description = "How much this trophy values. Defaults to 10"]
    #[min = -999_999]
    #[max = 999_999]
    value: Option<i32>,
    #[description = "Dedicate the trophy to someone, defaults to no one. You can use an id or mention as well"]
    dedication: Option<String>,
    #[description = "If true, you'll sign this trophy as created by you. Defaults to false"]
    signed: Option<bool>,
    #[description = "The image for the trophy, only seen on showcase command"]
    image: Option<serenity::Attachment>,
    #[description = "Private details for the trophy, you can set why do you give the trophy here."]
    details: Option<String>,
) -> Result<(), Error> {
    let locale = util::locale(&ctx);
    let guild_id = util::require_guild_id(&ctx)?.get() as i64;
    let value = value.unwrap_or(DEFAULT_VALUE);

    // F3: every validation happens before any download or DB write.
    if let Err(err) = validate_fields(
        &name,
        description.as_deref(),
        emoji.as_deref(),
        value,
        dedication.as_deref(),
        details.as_deref(),
    ) {
        return util::reply_error(ctx, err.message(&locale), true).await;
    }

    let image_plan = match &image {
        None => None,
        Some(attachment) => match images::validate(attachment.content_type.as_deref(), attachment.size) {
            Ok(ext) => Some((attachment.url.clone(), ext)),
            Err(images::ImageError::UnsupportedType) => {
                return util::reply_error(ctx, i18n::t(&locale, "create-error-image-type"), true)
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
        },
    };

    let db = &ctx.data().db;
    if let Some(err) = precheck(db, guild_id, &name).await? {
        return util::reply_error(ctx, err.message(&locale), true).await;
    }

    // Download (slow path) only after all checks passed; defer so the
    // interaction never times out on a large attachment. Every error reply
    // past this point uses `reply_error_ephemeral`: the defer is PUBLIC (the
    // success embed is public), and a plain followup could not make the
    // error private (§2: all error replies are ephemeral).
    let trophy_id = Uuid::now_v7();
    let mut image_file: Option<(String, Vec<u8>)> = None;
    if let Some((url, ext)) = image_plan {
        ctx.defer().await?;
        let filename = images::filename(guild_id, trophy_id, ext);
        match images::download(&url, &filename).await {
            Ok(bytes) => image_file = Some((filename, bytes)),
            Err(err) => {
                log::warn!("/create image download failed (guild={guild_id}): {err:#}");
                return util::reply_error_ephemeral(
                    ctx,
                    i18n::t(&locale, "create-error-image-download"),
                )
                .await;
            }
        }
    }

    let (dedication_user_id, dedication_text) =
        resolve_dedication(&ctx, dedication.as_deref()).await;

    let new = NewTrophy {
        id: trophy_id,
        guild_id,
        creator_user_id: ctx.author().id.get() as i64,
        name: name.clone(),
        description: description.unwrap_or_else(|| DEFAULT_DESCRIPTION.to_string()),
        emoji: emoji.unwrap_or_else(|| DEFAULT_EMOJI.to_string()),
        value,
        image: image_file.as_ref().map(|(filename, _)| filename.clone()),
        dedication_user_id,
        dedication_text,
        details: details.unwrap_or_else(|| DEFAULT_DETAILS.to_string()),
        signed: signed.unwrap_or(false),
    };

    let trophy = match insert_trophy(db, new).await {
        Ok(Ok(model)) => model,
        Ok(Err(err)) => {
            if let Some((filename, _)) = &image_file {
                images::remove(filename).await;
            }
            // Race-window duplicate: may run after the public defer above.
            return util::reply_error_ephemeral(ctx, err.message(&locale)).await;
        }
        Err(err) => {
            if let Some((filename, _)) = &image_file {
                images::remove(filename).await;
            }
            return Err(err);
        }
    };

    // Success embed: emoji/name title, description, value, optional signature
    // and dedication fields; footer shows the name (ADR 0004: IDs are never
    // user-facing).
    let mut embed = serenity::CreateEmbed::new()
        .title(format!("{} {}", trophy.emoji, trophy.name))
        .description(trophy.description.clone())
        .colour(util::COLOR_SUCCESS)
        .field(
            i18n::t(&locale, "create-field-value"),
            i18n::t_args(&locale, "create-value", &[("value", i64::from(trophy.value).into())]),
            true,
        )
        .footer(serenity::CreateEmbedFooter::new(i18n::t_args(
            &locale,
            "create-footer",
            &[("name", trophy.name.clone().into())],
        )));

    if trophy.signed
        && let Some(creator) = trophy.creator_user_id
    {
        embed = embed.field(
            i18n::t(&locale, "create-field-signed"),
            format!("<@{creator}>"),
            true,
        );
    }
    let dedication_display = match (trophy.dedication_user_id, &trophy.dedication_text) {
        (Some(id), _) => Some(format!("<@{id}>")),
        (None, Some(text)) => Some(text.clone()),
        (None, None) => None,
    };
    if let Some(dedicated_to) = dedication_display {
        embed = embed.field(i18n::t(&locale, "create-field-dedicated"), dedicated_to, true);
    }

    let mut reply = poise::CreateReply::default().ephemeral(false);
    if let Some((filename, bytes)) = image_file {
        // Upload the downloaded bytes and point the embed image at them.
        embed = embed.attachment(filename.clone());
        reply = reply.attachment(serenity::CreateAttachment::bytes(bytes, filename));
    }
    ctx.send(reply.embed(embed)).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::test_support::{fresh_db, insert_guild};

    // --- validate_fields ---

    fn ok_fields() -> Result<(), CreateError> {
        validate_fields("Gold Medal", Some("desc"), Some("🏆"), 10, Some("ana"), Some("details"))
    }

    #[test]
    fn valid_fields_pass() {
        assert_eq!(ok_fields(), Ok(()));
        // All optionals absent.
        assert_eq!(validate_fields("x", None, None, 0, None, None), Ok(()));
    }

    #[test]
    fn empty_or_whitespace_name_is_rejected() {
        // Regression: a blank name persists with normalized_name == "" and,
        // worse, becomes an empty autocomplete choice label — which Discord
        // rejects (HTTP 400), breaking trophy autocomplete for EVERY command
        // in the guild until the row is removed. Must be refused server-side
        // (Discord options carry no min_length).
        assert_eq!(
            validate_fields("", None, None, 10, None, None),
            Err(CreateError::EmptyName)
        );
        assert_eq!(
            validate_fields("   ", None, None, 10, None, None),
            Err(CreateError::EmptyName),
            "whitespace-only also normalizes to empty"
        );
        assert_eq!(
            validate_fields("\t\n ", None, None, 10, None, None),
            Err(CreateError::EmptyName)
        );
        // A single visible character is still a valid name.
        assert_eq!(validate_fields("x", None, None, 10, None, None), Ok(()));
        // Punctuation/emoji-only names are unusual but addressable (they do
        // NOT normalize to empty), so they remain allowed.
        assert_eq!(validate_fields("!!!", None, None, 10, None, None), Ok(()));
    }

    #[test]
    fn field_limits_are_inclusive() {
        let at = |n| "x".repeat(n);
        assert_eq!(validate_fields(&at(32), None, None, 10, None, None), Ok(()));
        assert_eq!(
            validate_fields(&at(33), None, None, 10, None, None),
            Err(CreateError::FieldTooLong { field: "name", max: 32 })
        );
        assert_eq!(validate_fields("n", Some(&at(128)), None, 10, None, None), Ok(()));
        assert_eq!(
            validate_fields("n", Some(&at(129)), None, 10, None, None),
            Err(CreateError::FieldTooLong { field: "description", max: 128 })
        );
        assert_eq!(validate_fields("n", None, Some(&at(64)), 10, None, None), Ok(()));
        assert_eq!(
            validate_fields("n", None, Some(&at(65)), 10, None, None),
            Err(CreateError::FieldTooLong { field: "emoji", max: 64 })
        );
        assert_eq!(validate_fields("n", None, None, 10, Some(&at(32)), None), Ok(()));
        assert_eq!(
            validate_fields("n", None, None, 10, Some(&at(33)), None),
            Err(CreateError::FieldTooLong { field: "dedication", max: 32 })
        );
        assert_eq!(validate_fields("n", None, None, 10, None, Some(&at(300))), Ok(()));
        assert_eq!(
            validate_fields("n", None, None, 10, None, Some(&at(301))),
            Err(CreateError::FieldTooLong { field: "details", max: 300 })
        );
    }

    #[test]
    fn limits_count_characters_not_bytes() {
        // 32 multibyte chars must pass (legacy JS counted UTF-16 units, but
        // chars are the closest server-side equivalent for user text).
        let name: String = "é".repeat(32);
        assert_eq!(validate_fields(&name, None, None, 10, None, None), Ok(()));
    }

    #[test]
    fn value_range_is_enforced_serverside() {
        assert_eq!(validate_fields("n", None, None, 999_999, None, None), Ok(()));
        assert_eq!(validate_fields("n", None, None, -999_999, None, None), Ok(()));
        assert_eq!(
            validate_fields("n", None, None, 1_000_000, None, None),
            Err(CreateError::ValueOutOfRange)
        );
        assert_eq!(
            validate_fields("n", None, None, -1_000_000, None, None),
            Err(CreateError::ValueOutOfRange)
        );
    }

    #[test]
    fn validation_follows_legacy_order() {
        // Legacy order: name before details before description.
        let long = "x".repeat(500);
        assert_eq!(
            validate_fields(&long, Some(&long), None, 10, None, Some(&long)),
            Err(CreateError::FieldTooLong { field: "name", max: 32 })
        );
        assert_eq!(
            validate_fields("n", Some(&long), None, 10, None, Some(&long)),
            Err(CreateError::FieldTooLong { field: "details", max: 300 })
        );
    }

    // --- parse_dedication ---

    #[test]
    fn dedication_mentions_and_snowflakes_become_user() {
        assert_eq!(parse_dedication("<@123456789012345678>"), Dedication::User(123456789012345678));
        assert_eq!(parse_dedication("<@!123456789012345678>"), Dedication::User(123456789012345678));
        assert_eq!(parse_dedication("123456789012345678"), Dedication::User(123456789012345678));
        assert_eq!(parse_dedication(" 123456789012345678 "), Dedication::User(123456789012345678));
    }

    #[test]
    fn dedication_anything_else_is_text() {
        assert_eq!(parse_dedication("John the Great"), Dedication::Text("John the Great".into()));
        // Too short to be a snowflake.
        assert_eq!(parse_dedication("123"), Dedication::Text("123".into()));
        // Digits mixed with letters.
        assert_eq!(parse_dedication("abc123456789012345"), Dedication::Text("abc123456789012345".into()));
        // Malformed mention.
        assert_eq!(parse_dedication("<@abc>"), Dedication::Text("<@abc>".into()));
        // Text is preserved verbatim (untouched by the trim used for parsing).
        assert_eq!(parse_dedication(" mom "), Dedication::Text(" mom ".into()));
    }

    /// Regression guard (§2 "all error replies are ephemeral"): the image
    /// path defers PUBLICLY, and Discord locks visibility at defer time — so
    /// every error reply issued after the defer must go through
    /// `reply_error_ephemeral` (which deletes the public placeholder first),
    /// never the plain `reply_error`. Checked on the source because poise
    /// contexts aren't mockable (same pattern as `src/bot/buttons.rs`).
    #[test]
    fn errors_after_the_public_defer_are_ephemeral() {
        let src = include_str!("create.rs");
        // Handler body only (the segment ends at this very test's literal).
        let handler = src.split("pub async fn create").nth(1).expect("handler exists");
        let defer = handler.find("ctx.defer()").expect("image path defers");
        // Built via concat! so this test's own source cannot match.
        let plain_reply_error = concat!("util::reply_", "error(");
        assert!(
            !handler[defer..].contains(plain_reply_error),
            "an error path after ctx.defer() uses the plain reply_error; \
             use util::reply_error_ephemeral instead"
        );
    }

    // --- dedication_columns (fetch-failure parity) ---

    #[test]
    fn resolved_user_dedication_stores_id_and_name_snapshot() {
        let raw = "<@123456789012345678>";
        assert_eq!(
            dedication_columns(raw, parse_dedication(raw), Some("ana".into())),
            (Some(123456789012345678), Some("ana".to_string()))
        );
    }

    #[test]
    fn unresolvable_user_dedication_falls_back_to_the_raw_text() {
        // Legacy parity (globals.js parseUser): a well-formed but
        // unresolvable mention/snowflake (typo, deleted account) is stored
        // as a TEXT dedication with the raw input — NOT as a user
        // dedication with a NULL text snapshot (which would render a broken
        // <@…> mention in every display mode).
        for raw in ["123456789012345678", "<@123456789012345678>"] {
            assert_eq!(
                dedication_columns(raw, parse_dedication(raw), None),
                (None, Some(raw.to_string())),
                "raw: {raw:?}"
            );
        }
    }

    #[test]
    fn text_dedication_ignores_any_fetched_name() {
        assert_eq!(
            dedication_columns("mom", parse_dedication("mom"), None),
            (None, Some("mom".to_string()))
        );
    }

    // --- i18n catalog ---

    #[test]
    fn all_create_messages_exist() {
        let locale = i18n::resolve(None);
        // Messages with placeables need their args supplied, otherwise fluent
        // reports an error and the lookup falls back to the key.
        let args: &[(&'static str, i18n::FluentValue<'static>)] = &[
            ("max", 32.into()),
            ("min", (-32).into()),
            ("name", "Gold".into()),
            ("value", 10.into()),
        ];
        for key in [
            "create-error-name-too-long",
            "create-error-details-too-long",
            "create-error-description-too-long",
            "create-error-emoji-too-long",
            "create-error-dedication-too-long",
            "create-error-value-out-of-range",
            "create-error-guild-full",
            "create-error-duplicate-name",
            "create-error-image-type",
            "create-error-image-too-large",
            "create-error-image-download",
            "create-field-value",
            "create-value",
            "create-field-signed",
            "create-field-dedicated",
            "create-footer",
        ] {
            assert_ne!(i18n::t_args(&locale, key, args), key, "missing ftl message: {key}");
        }
    }

    #[test]
    fn error_messages_render_their_arguments() {
        let locale = i18n::resolve(None);
        let message = CreateError::FieldTooLong { field: "name", max: 32 }.message(&locale);
        assert!(message.contains("32"), "got: {message}");
        let message = CreateError::GuildFull.message(&locale);
        assert!(message.contains("150"), "got: {message}");
        let message = CreateError::DuplicateName { name: "Gold".into() }.message(&locale);
        assert!(message.contains("Gold"), "got: {message}");
    }

    // --- DB integration ---

    fn new_trophy(guild_id: i64, name: &str) -> NewTrophy {
        NewTrophy {
            id: Uuid::now_v7(),
            guild_id,
            creator_user_id: 7,
            name: name.to_string(),
            description: DEFAULT_DESCRIPTION.to_string(),
            emoji: DEFAULT_EMOJI.to_string(),
            value: DEFAULT_VALUE,
            image: None,
            dedication_user_id: None,
            dedication_text: None,
            details: DEFAULT_DETAILS.to_string(),
            signed: false,
        }
    }

    #[tokio::test]
    async fn insert_trophy_creates_guild_row_and_stores_all_fields() {
        let db = fresh_db().await;

        let new = NewTrophy {
            value: -5,
            image: Some("1_x.png".into()),
            dedication_user_id: Some(42),
            dedication_text: Some("ana".into()),
            signed: true,
            ..new_trophy(1, "Gold Medal")
        };
        let id = new.id;
        let model = insert_trophy(&db, new).await.expect("infra ok").expect("inserted");

        assert_eq!(model.id, id);
        assert_eq!(model.guild_id, 1);
        assert_eq!(model.legacy_id, None, "post-cutover trophies carry no legacy id");
        assert_eq!(model.creator_user_id, Some(7));
        assert_eq!(model.name, "Gold Medal");
        assert_eq!(model.normalized_name, "goldmedal");
        assert_eq!(model.value, -5);
        assert_eq!(model.image.as_deref(), Some("1_x.png"));
        assert_eq!(model.dedication_user_id, Some(42));
        assert_eq!(model.dedication_text.as_deref(), Some("ana"));
        assert!(model.signed);

        // The guild row was auto-created (FK satisfied), not safe by default.
        let guild = guilds::Entity::find_by_id(1i64).one(&db).await.unwrap().unwrap();
        assert!(!guild.is_safe);
    }

    #[tokio::test]
    async fn insert_trophy_does_not_clobber_an_existing_guild_row() {
        let db = fresh_db().await;
        insert_guild(&db, 1).await; // test_support inserts with is_safe = true

        insert_trophy(&db, new_trophy(1, "Gold")).await.unwrap().unwrap();

        let guild = guilds::Entity::find_by_id(1i64).one(&db).await.unwrap().unwrap();
        assert!(guild.is_safe, "upsert must not overwrite the existing guild row");
    }

    #[tokio::test]
    async fn insert_trophy_race_duplicate_maps_to_duplicate_name() {
        let db = fresh_db().await;

        insert_trophy(&db, new_trophy(1, "Gold Medal")).await.unwrap().unwrap();
        // Same normalized name ("gold-medal!" → "goldmedal") hits the unique
        // index — the race-window path precheck cannot cover.
        let result = insert_trophy(&db, new_trophy(1, "GOLD! medal")).await.expect("infra ok");
        assert_eq!(result, Err(CreateError::DuplicateName { name: "GOLD! medal".into() }));

        // Same name in ANOTHER guild is fine (uniqueness is per guild).
        insert_trophy(&db, new_trophy(2, "Gold Medal")).await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn precheck_detects_normalized_duplicates() {
        let db = fresh_db().await;
        insert_trophy(&db, new_trophy(1, "Gold Medal")).await.unwrap().unwrap();

        assert_eq!(precheck(&db, 1, "Fresh Name").await.unwrap(), None);
        assert_eq!(
            precheck(&db, 1, "gold medal!").await.unwrap(),
            Some(CreateError::DuplicateName { name: "gold medal!".into() })
        );
        // Other guilds are unaffected.
        assert_eq!(precheck(&db, 2, "gold medal!").await.unwrap(), None);
    }

    #[tokio::test]
    async fn precheck_enforces_the_guild_capacity() {
        let db = fresh_db().await;
        for i in 0..MAX_TROPHIES_PER_GUILD {
            insert_trophy(&db, new_trophy(1, &format!("Trophy {i}")))
                .await
                .unwrap()
                .unwrap();
        }

        assert_eq!(precheck(&db, 1, "One More").await.unwrap(), Some(CreateError::GuildFull));
        // Capacity is checked before duplicates (legacy order) and per guild.
        assert_eq!(precheck(&db, 1, "Trophy 0").await.unwrap(), Some(CreateError::GuildFull));
        assert_eq!(precheck(&db, 2, "One More").await.unwrap(), None);
    }

    #[test]
    fn trophy_model_partialeq_helper_compiles() {
        // Guard: CreateError must stay PartialEq for the assertions above.
        assert_eq!(CreateError::GuildFull, CreateError::GuildFull);
    }
}
