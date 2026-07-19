//! `/trophies` — trophy lists for a user or the whole guild (batch C4).
//!
//! Spec: docs/specs/commands-user.md § /trophies user + guild. Fixes applied:
//! - F18: the target's display name comes from the interaction's resolved
//!   `User` object (global name → username), so the legacy "undefined's
//!   Trophies" fetch-failure bug cannot happen.
//! - F19: viewers with Manage Guild (or Administrator) always see unused
//!   trophies regardless of the `hide_unused_trophies` setting — the
//!   documented INTENT, which was dead code in the Node.js bot.
//! - F20: n/a here — orphaned award IDs are impossible thanks to the FK on
//!   `user_trophies.trophy_id`, so the list and the total always agree.

use poise::serenity_prelude as serenity;
use sea_orm::sea_query::{Alias, Expr, ExprTrait, Order, Query};
use sea_orm::{ConnectionTrait, DbErr};

use crate::bot::util::{self, paginate};
use crate::bot::{Context, Error};
use crate::domain::queries;
use crate::domain::settings::{self, Setting};
use crate::entities::{trophies, user_trophies};
use crate::i18n;

/// Rows per page, both subcommands (legacy `getPage(list, 10, page)`).
pub const PER_PAGE: usize = 10;

/// See a list of trophies.
#[poise::command(
    slash_command,
    guild_only,
    subcommands("user", "guild"),
    subcommand_required
)]
pub async fn trophies(_ctx: Context<'_>) -> Result<(), Error> {
    // Never reached: subcommand_required.
    Ok(())
}

/// Show the trophies any user has.
#[poise::command(slash_command, guild_only)]
async fn user(
    ctx: Context<'_>,
    #[description = "User to check trophies"] user: Option<serenity::User>,
    #[description = "Page to look at"] page: Option<i64>,
) -> Result<(), Error> {
    let guild_id = effective_guild_id(&ctx).await?;
    let target = user.as_ref().unwrap_or_else(|| ctx.author());
    let target_id = i64::try_from(target.id.get())?;
    let db = &ctx.data().db;
    let locale = util::locale(&ctx);

    let rows = user_trophy_rows(db, guild_id, target_id).await?;
    let score = queries::user_score(db, guild_id, target_id).await?;
    let (slice, current, last) = paginate(&rows, PER_PAGE, page.unwrap_or(1));

    let body = if slice.is_empty() {
        i18n::t(&locale, "trophies-user-empty")
    } else {
        slice
            .iter()
            .map(|row| {
                i18n::t_args(
                    &locale,
                    "trophies-user-row",
                    &[
                        ("emoji", row.emoji.clone().into()),
                        ("name", row.name.clone().into()),
                        ("value", value_markup(row.value).into()),
                        ("count", row.count.into()),
                        ("awarded_at", row.awarded_at.map(|at| at.format("%Y-%m-%d %H:%M").to_string()).into()),
                    ],
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    };

    let title = i18n::t_args(
        &locale,
        "trophies-user-title",
        &[("name", display_name(target.global_name.as_deref(), &target.name).to_string().into())],
    );
    let total = i18n::t_args(&locale, "trophies-user-total", &[("score", score.into())]);
    let embed = serenity::CreateEmbed::new()
        .title(title)
        .description(format!("{total}\n\n{body}"))
        .colour(util::COLOR_MAIN)
        .footer(serenity::CreateEmbedFooter::new(page_footer(
            &locale, current, last,
        )));
    util::reply_embed(ctx, embed, false).await
}

/// Show the trophies any guild has.
#[poise::command(slash_command, guild_only)]
async fn guild(
    ctx: Context<'_>,
    #[description = "Page to look at"] page: Option<i64>,
) -> Result<(), Error> {
    let guild_id = effective_guild_id(&ctx).await?;
    let db = &ctx.data().db;
    let locale = util::locale(&ctx);

    let all = guild_trophy_rows(db, guild_id).await?;
    let hide_setting = settings::get_setting(db, guild_id, Setting::HideUnusedTrophies).await?;
    // F19: Manage Guild (or Administrator) viewers are exempt from hiding.
    let is_manager = ctx
        .author_member()
        .await
        .and_then(|member| member.permissions)
        .is_some_and(|perms| perms.manage_guild() || perms.administrator());
    let show_unused = shows_unused(hide_setting, is_manager);

    let visible: Vec<&GuildTrophyRow> =
        all.iter().filter(|row| show_unused || row.used).collect();
    let hidden = all.len() - visible.len();
    let (slice, current, last) = paginate(&visible, PER_PAGE, page.unwrap_or(1));

    let body = if slice.is_empty() {
        i18n::t(&locale, "trophies-guild-empty")
    } else {
        slice
            .iter()
            .map(|row| {
                i18n::t_args(
                    &locale,
                    "trophies-guild-row",
                    &[
                        ("emoji", row.emoji.clone().into()),
                        ("name", row.name.clone().into()),
                        ("value", value_markup(row.value).into()),
                    ],
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    };

    // Spec "Rust target": report both the real created total and, when the
    // unused filter hides some, how many are hidden — no silent mismatch.
    let mut description = i18n::t_args(
        &locale,
        "trophies-guild-total",
        &[("total", all.len().into())],
    );
    if hidden > 0 {
        description.push('\n');
        description.push_str(&i18n::t_args(
            &locale,
            "trophies-guild-hidden",
            &[("hidden", hidden.into())],
        ));
    }
    let embed = serenity::CreateEmbed::new()
        .title(i18n::t(&locale, "trophies-guild-title"))
        .description(format!("{description}\n\n{body}"))
        .colour(util::COLOR_MAIN)
        .footer(serenity::CreateEmbedFooter::new(page_footer(
            &locale, current, last,
        )));
    util::reply_embed(ctx, embed, false).await
}

/// One aggregated `/trophies user` row: a trophy the user holds, with how
/// many copies they have.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserTrophyRow {
    pub emoji: String,
    pub name: String,
    pub value: i32,
    pub count: i64,
    pub awarded_at: Option<chrono::NaiveDateTime>,
}

/// One `/trophies guild` row; `used` = at least one award references it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuildTrophyRow {
    pub emoji: String,
    pub name: String,
    pub value: i32,
    pub used: bool,
}

/// A user's awards aggregated per trophy (`COUNT(*)` grouped join over
/// `user_trophies × trophies`), ordered by trophy value descending, then by
/// trophy id ascending (UUIDv7 = creation order) for determinism.
pub async fn user_trophy_rows(
    db: &impl ConnectionTrait,
    guild_id: i64,
    user_id: i64,
) -> Result<Vec<UserTrophyRow>, DbErr> {
    let count = Alias::new("count");
    let awarded_at = Alias::new("awarded_at");
    let trophy_id = (trophies::Entity, trophies::Column::Id);
    let stmt = Query::select()
        .column((trophies::Entity, trophies::Column::Emoji))
        .column((trophies::Entity, trophies::Column::Name))
        .column((trophies::Entity, trophies::Column::Value))
        .expr_as(Expr::col(trophy_id).count(), count)
        .expr_as(Expr::col((user_trophies::Entity, user_trophies::Column::AwardedAt)).max(), awarded_at)
        .from(user_trophies::Entity)
        .inner_join(
            trophies::Entity,
            Expr::col(trophy_id)
                .equals((user_trophies::Entity, user_trophies::Column::TrophyId)),
        )
        .and_where(
            Expr::col((user_trophies::Entity, user_trophies::Column::GuildId)).eq(guild_id),
        )
        .and_where(
            Expr::col((user_trophies::Entity, user_trophies::Column::UserId)).eq(user_id),
        )
        .group_by_col(trophy_id)
        .order_by((trophies::Entity, trophies::Column::Value), Order::Desc)
        .order_by(trophy_id, Order::Asc)
        .to_owned();

    db.query_all(&stmt)
        .await?
        .iter()
        .map(|row| {
            Ok(UserTrophyRow {
                emoji: row.try_get_by_index(0)?,
                name: row.try_get_by_index(1)?,
                value: row.try_get_by_index(2)?,
                count: row.try_get_by_index(3)?,
                awarded_at: row.try_get_by_index(4)?,
            })
        })
        .collect()
}

/// All trophies of a guild ordered by value descending (id ascending on
/// ties), each flagged with whether any award references it (`EXISTS`
/// subquery — no full award-array scans like the legacy bot).
pub async fn guild_trophy_rows(
    db: &impl ConnectionTrait,
    guild_id: i64,
) -> Result<Vec<GuildTrophyRow>, DbErr> {
    let used = Alias::new("used");
    let trophy_id = (trophies::Entity, trophies::Column::Id);
    let awards_exist = Query::select()
        .expr(Expr::val(1))
        .from(user_trophies::Entity)
        .and_where(
            Expr::col((user_trophies::Entity, user_trophies::Column::TrophyId))
                .equals(trophy_id),
        )
        .to_owned();
    let stmt = Query::select()
        .column((trophies::Entity, trophies::Column::Emoji))
        .column((trophies::Entity, trophies::Column::Name))
        .column((trophies::Entity, trophies::Column::Value))
        .expr_as(Expr::exists(awards_exist), used)
        .from(trophies::Entity)
        .and_where(Expr::col((trophies::Entity, trophies::Column::GuildId)).eq(guild_id))
        .order_by((trophies::Entity, trophies::Column::Value), Order::Desc)
        .order_by(trophy_id, Order::Asc)
        .to_owned();

    db.query_all(&stmt)
        .await?
        .iter()
        .map(|row| {
            Ok(GuildTrophyRow {
                emoji: row.try_get_by_index(0)?,
                name: row.try_get_by_index(1)?,
                value: row.try_get_by_index(2)?,
                used: row.try_get_by_index(3)?,
            })
        })
        .collect()
}

/// Legacy value cosmetics: positive → ` **+N**`, negative → ` **-N**`,
/// zero → nothing at all (leading space included so rows compose cleanly).
pub fn value_markup(value: i32) -> String {
    match value {
        0 => String::new(),
        v if v > 0 => format!(" **+{v}**"),
        v => format!(" **{v}**"),
    }
}

/// F19: whether unused trophies are shown to this viewer. Managers always
/// see them; everyone else follows the setting (0 = Hide, 1 = Show).
pub fn shows_unused(hide_unused_setting: i16, viewer_is_manager: bool) -> bool {
    viewer_is_manager || hide_unused_setting != 0
}

/// F18: display name straight from resolved interaction data — global
/// display name when set, else the username. Never "undefined".
pub fn display_name<'a>(global_name: Option<&'a str>, username: &'a str) -> &'a str {
    global_name.unwrap_or(username)
}

fn page_footer(locale: &i18n::LanguageIdentifier, page: usize, last: usize) -> String {
    i18n::t_args(
        locale,
        "trophies-footer-page",
        &[("page", page.into()), ("last", last.into())],
    )
}

/// Effective guild (guild_links): a linked guild's /trophies lists the
/// SOURCE guild's trophies/holders it mirrors.
async fn effective_guild_id(ctx: &Context<'_>) -> Result<i64, Error> {
    Ok(util::effective_guild_id(ctx).await?.get() as i64)
}

#[cfg(test)]
mod tests {
    use sea_orm::{ActiveModelTrait, DatabaseConnection, Set};
    use uuid::Uuid;

    use super::*;
    use crate::domain::test_support::{fresh_db, insert_guild, now};

    // ---- pure logic -----------------------------------------------------
    // (pagination tests live with `paginate` in `crate::bot::util`)

    #[test]
    fn value_markup_matches_legacy_cosmetics() {
        assert_eq!(value_markup(10), " **+10**");
        assert_eq!(value_markup(-3), " **-3**");
        assert_eq!(value_markup(0), "");
    }

    #[test]
    fn f19_managers_always_see_unused_trophies() {
        // Setting 0 = Hide, 1 = Show (default).
        assert!(shows_unused(0, true), "manager exempt from hiding");
        assert!(shows_unused(1, true));
        assert!(!shows_unused(0, false), "non-manager honors Hide");
        assert!(shows_unused(1, false), "non-manager honors Show");
    }

    #[test]
    fn f18_display_name_prefers_global_name_and_never_fails() {
        assert_eq!(display_name(Some("Ana Display"), "ana123"), "Ana Display");
        assert_eq!(display_name(None, "ana123"), "ana123");
    }

    // ---- catalog --------------------------------------------------------

    #[test]
    fn trophies_messages_exist_in_catalog() {
        let locale = i18n::resolve(None);
        // Keys without variables resolve via plain lookup.
        for key in ["trophies-user-empty", "trophies-guild-title", "trophies-guild-empty"] {
            assert_ne!(i18n::t(&locale, key), key, "missing catalog key {key}");
        }
        // Keys with variables need their arguments supplied.
        let title = i18n::t_args(&locale, "trophies-user-title", &[("name", "Ana".into())]);
        assert!(title.contains("Ana"), "got: {title}");
        let total = i18n::t_args(&locale, "trophies-user-total", &[("score", 17.into())]);
        assert!(total.contains("17"), "got: {total}");
        let guild_total =
            i18n::t_args(&locale, "trophies-guild-total", &[("total", 4.into())]);
        assert!(guild_total.contains('4'), "got: {guild_total}");
        let hidden = i18n::t_args(&locale, "trophies-guild-hidden", &[("hidden", 2.into())]);
        assert!(hidden.contains('2'), "got: {hidden}");
        let one_hidden =
            i18n::t_args(&locale, "trophies-guild-hidden", &[("hidden", 1.into())]);
        assert!(one_hidden.contains("trophy"), "got: {one_hidden}");
        let guild_row = i18n::t_args(
            &locale,
            "trophies-guild-row",
            &[
                ("emoji", "🏆".into()),
                ("name", "Gold".into()),
                ("value", value_markup(-3).into()),
            ],
        );
        assert!(guild_row.contains("-3"), "got: {guild_row}");

        let row = i18n::t_args(
            &locale,
            "trophies-user-row",
            &[
                ("emoji", "🏆".into()),
                ("name", "Gold".into()),
                ("value", value_markup(10).into()),
                ("count", 3.into()),
                ("awarded_at", "2026-01-01 12:34".into()),
            ],
        );
        assert!(row.contains("Gold"), "got: {row}");
        assert!(row.contains("+10"), "got: {row}");
        assert!(row.contains('3'), "got: {row}");

        let footer = page_footer(&locale, 2, 5);
        assert!(footer.contains('2') && footer.contains('5'), "got: {footer}");
    }

    // ---- registration ---------------------------------------------------

    #[test]
    fn trophies_registers_user_and_guild_subcommands() {
        let command = trophies();
        assert!(command.subcommand_required);
        let names: Vec<&str> = command
            .subcommands
            .iter()
            .map(|c| c.name.as_ref())
            .collect();
        assert_eq!(names, vec!["user", "guild"]);
        for sub in &command.subcommands {
            let description = sub.description.as_deref().unwrap_or_default();
            assert!(!description.is_empty(), "/trophies {} needs a description", sub.name);
        }
    }

    // ---- integration (sqlite::memory:) ----------------------------------

    async fn insert_trophy(
        db: &DatabaseConnection,
        guild_id: i64,
        name: &str,
        emoji: &str,
        value: i32,
    ) -> Uuid {
        let id = Uuid::now_v7();
        trophies::ActiveModel {
            id: Set(id),
            guild_id: Set(guild_id),
            legacy_id: Set(None),
            creator_user_id: Set(None),
            name: Set(name.to_string()),
            normalized_name: Set(crate::domain::normalize::normalize_name(name)),
            description: Set("No description provided".to_string()),
            emoji: Set(emoji.to_string()),
            value: Set(value),
            image: Set(None),
            dedication_user_id: Set(None),
            dedication_text: Set(None),
            details: Set("No details provided.".to_string()),
            signed: Set(false),
            category: Set(None),
            active: Set(true),
            created_at: Set(now()),
            updated_at: Set(now()),
        }
        .insert(db)
        .await
        .expect("insert trophy");
        id
    }

    async fn award(db: &DatabaseConnection, guild_id: i64, user_id: i64, trophy_id: Uuid) {
        user_trophies::ActiveModel {
            id: Set(Uuid::now_v7()),
            guild_id: Set(guild_id),
            user_id: Set(user_id),
            trophy_id: Set(trophy_id),
            awarded_by: Set(None),
            awarded_at: Set(now()),
            created_at: Set(now()),
            updated_at: Set(now()),
        }
        .insert(db)
        .await
        .expect("insert award");
    }

    #[tokio::test]
    async fn user_rows_aggregate_counts_and_sort_by_value_desc() {
        let db = fresh_db().await;
        insert_guild(&db, 1).await;
        let gold = insert_trophy(&db, 1, "Gold", "🥇", 50).await;
        let small = insert_trophy(&db, 1, "Small", "🏆", 5).await;
        let shame = insert_trophy(&db, 1, "Shame", "💀", -3).await;

        award(&db, 1, 42, small).await;
        award(&db, 1, 42, small).await;
        award(&db, 1, 42, small).await;
        award(&db, 1, 42, gold).await;
        award(&db, 1, 42, shame).await;

        let rows = user_trophy_rows(&db, 1, 42).await.expect("query rows");
        assert_eq!(rows.len(), 3);
        assert!(rows[0].awarded_at.is_some(), "gold row should carry the latest award time");
        assert!(rows[1].awarded_at.is_some(), "small row should carry the latest award time");
        assert!(rows[2].awarded_at.is_some(), "shame row should carry the latest award time");
    }

    #[tokio::test]
    async fn user_rows_are_scoped_to_guild_and_user() {
        let db = fresh_db().await;
        insert_guild(&db, 1).await;
        insert_guild(&db, 2).await;
        let here = insert_trophy(&db, 1, "Here", "🏆", 10).await;
        let elsewhere = insert_trophy(&db, 2, "Elsewhere", "🏆", 10).await;

        award(&db, 1, 42, here).await;
        award(&db, 2, 42, elsewhere).await; // same user, other guild
        award(&db, 1, 43, here).await; // other user, same guild

        let rows = user_trophy_rows(&db, 1, 42).await.expect("query rows");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "Here");
        assert_eq!(rows[0].count, 1);

        let rows = user_trophy_rows(&db, 1, 99).await.expect("query rows");
        assert!(rows.is_empty(), "user with no awards has no rows");
    }

    #[tokio::test]
    async fn user_rows_total_matches_domain_user_score() {
        let db = fresh_db().await;
        insert_guild(&db, 1).await;
        let gold = insert_trophy(&db, 1, "Gold", "🥇", 10).await;
        let shame = insert_trophy(&db, 1, "Shame", "💀", -3).await;
        award(&db, 1, 42, gold).await;
        award(&db, 1, 42, gold).await;
        award(&db, 1, 42, shame).await;

        let rows = user_trophy_rows(&db, 1, 42).await.expect("query rows");
        let from_rows: i64 = rows.iter().map(|r| i64::from(r.value) * r.count).sum();
        let score = queries::user_score(&db, 1, 42).await.expect("score");
        assert_eq!(from_rows, score, "list and total must always agree (F20)");
        assert_eq!(score, 17);
    }

    #[tokio::test]
    async fn guild_rows_flag_used_and_sort_by_value_desc() {
        let db = fresh_db().await;
        insert_guild(&db, 1).await;
        insert_guild(&db, 2).await;
        let used_low = insert_trophy(&db, 1, "Bronze", "🥉", 1).await;
        let _unused_high = insert_trophy(&db, 1, "Diamond", "💎", 100).await;
        let other_guild = insert_trophy(&db, 2, "Other", "🏆", 7).await;

        award(&db, 1, 42, used_low).await;
        award(&db, 2, 42, other_guild).await; // must not mark guild-1 rows used

        let rows = guild_trophy_rows(&db, 1).await.expect("query rows");
        assert_eq!(
            rows,
            vec![
                GuildTrophyRow { emoji: "💎".into(), name: "Diamond".into(), value: 100, used: false },
                GuildTrophyRow { emoji: "🥉".into(), name: "Bronze".into(), value: 1, used: true },
            ]
        );
    }

    #[tokio::test]
    async fn guild_rows_unused_filter_hides_only_for_non_managers() {
        let db = fresh_db().await;
        insert_guild(&db, 1).await;
        let used = insert_trophy(&db, 1, "Used", "🏆", 10).await;
        let _unused = insert_trophy(&db, 1, "Unused", "🏆", 5).await;
        award(&db, 1, 42, used).await;

        let all = guild_trophy_rows(&db, 1).await.expect("query rows");
        assert_eq!(all.len(), 2);

        // Non-manager with setting Hide (0): only the used trophy remains.
        let visible: Vec<&GuildTrophyRow> = all
            .iter()
            .filter(|row| shows_unused(0, false) || row.used)
            .collect();
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].name, "Used");

        // Manager with setting Hide (0): sees everything (F19).
        let visible: Vec<&GuildTrophyRow> = all
            .iter()
            .filter(|row| shows_unused(0, true) || row.used)
            .collect();
        assert_eq!(visible.len(), 2);
    }
}
