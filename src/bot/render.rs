//! Shared leaderboard renderer, used by `/leaderboard` and (later) the
//! persistent panel updater — one code path, no `updatePanel` duplication
//! (spec: docs/specs/commands-user.md § /leaderboard "Rust target").
//!
//! Fixes implemented here:
//! - F13: a departed user never crashes rendering — name formats 1-3 fall
//!   back to a mention when the member cannot be resolved.
//! - F14: the requested page is clamped BEFORE rank math, so rank numbers
//!   and medals always match the rows shown and the footer.
//! - F15: users with at least one award appear even at score 0 (the DB
//!   query already includes them), and "Total server score" is the real
//!   whole-server aggregate, independent of display filters.
//! - F16: membership is resolved explicitly per user (cache first, HTTP
//!   fallback) with a documented fallback: a lookup that fails for any
//!   reason other than "not a member" counts as present, so transient
//!   errors never hide users.
//!
//! Lookup cost is BOUNDED (spec: "HTTP fallback per page (only 10
//! lookups)"): the quit-user filter walks the board in rank order and stops
//! as soon as the requested page is filled, never exceeding
//! [`MEMBERSHIP_LOOKUP_BUDGET`] HTTP calls; name formats resolve only the
//! displayed page (≤ [`PER_PAGE`] more). Users left unresolved default to
//! [`Membership::Unknown`] and stay visible — on a board where everyone is
//! still a member, a page-1 render (the panel updater's case) costs at most
//! 10 lookups total.

use std::collections::HashMap;

use poise::serenity_prelude as serenity;
use sea_orm::ConnectionTrait;

use crate::bot::util;
use crate::domain::{queries, settings};
use crate::i18n::{self, LanguageIdentifier};

/// Users per leaderboard page (legacy parity).
pub const PER_PAGE: usize = 10;

/// Hard upper bound on membership HTTP lookups spent filtering quit users
/// in a single render. The filter walk stops early once the requested page
/// is filled, so this cap only bites on boards where many top-ranked users
/// have left; users beyond it stay unresolved ([`Membership::Unknown`],
/// visible). Keeps `/leaderboard` and every panel refresh rate-limit-safe
/// even on production-size guilds.
pub const MEMBERSHIP_LOOKUP_BUDGET: usize = 50;

/// Resolved membership of a leaderboard user in the guild (F16).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Membership {
    /// Confirmed member, with the fields the display formats need.
    Present {
        username: String,
        nickname: Option<String>,
        tag: String,
    },
    /// Confirmed NOT a member (Discord answered 404 / unknown member).
    Absent,
    /// Lookup failed for another reason (network, permissions). Treated as
    /// present for filtering — never hide someone over a transient error —
    /// and rendered as a mention (F13).
    Unknown,
}

/// The `leaderboard_format` setting as a type. Unknown stored values fall
/// back gracefully to `Mention` (the legacy `default:` switch arm).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeaderboardFormat {
    /// 0 — `<@id>`, no member data needed.
    Mention,
    /// 1 — username.
    Username,
    /// 2 — guild nickname, falling back to username.
    Nickname,
    /// 3 — username and tag.
    UsernameTag,
}

impl LeaderboardFormat {
    pub fn from_setting(value: i16) -> Self {
        match value {
            1 => Self::Username,
            2 => Self::Nickname,
            3 => Self::UsernameTag,
            _ => Self::Mention,
        }
    }
}

/// Clamped pagination bounds (F14): `page` is always within `[1, last]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PageBounds {
    pub page: usize,
    pub last: usize,
}

/// Clamps a requested page (raw slash-command input: may be 0, negative or
/// past the end) into valid bounds for a list of `len` entries.
pub fn clamp_page(requested: i64, len: usize) -> PageBounds {
    let last = len.div_ceil(PER_PAGE).max(1);
    let page = requested.clamp(1, last as i64) as usize;
    PageBounds { page, last }
}

/// Medal for a global rank: 🥇🥈🥉 for 1-3, generic medal otherwise
/// (legacy `getMedal`).
pub fn medal(rank: usize) -> &'static str {
    match rank {
        1 => "🥇",
        2 => "🥈",
        3 => "🥉",
        _ => ":medal:",
    }
}

/// Renders one user's display name for the configured format. Formats that
/// need member data fall back to a mention when the member is absent or
/// unresolved — Discord renders a readable placeholder for those, and the
/// command never crashes (F13).
pub fn display_name(format: LeaderboardFormat, user_id: i64, membership: &Membership) -> String {
    let mention = format!("<@{user_id}>");
    let Membership::Present {
        username,
        nickname,
        tag,
    } = membership
    else {
        return mention;
    };

    match format {
        LeaderboardFormat::Mention => mention,
        LeaderboardFormat::Username => username.clone(),
        LeaderboardFormat::Nickname => nickname.clone().unwrap_or_else(|| username.clone()),
        LeaderboardFormat::UsernameTag => tag.clone(),
    }
}

/// A fully computed leaderboard page, ready to be put into an embed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoardView {
    /// Real whole-server score: sum over ALL users with awards, independent
    /// of visibility filters (F15).
    pub total_score: i64,
    /// Formatted rows for the (clamped) current page.
    pub rows: Vec<String>,
    /// Clamped current page (1-based).
    pub page: usize,
    /// Last page (at least 1).
    pub last: usize,
}

/// The board rows that survive the quit-user filter, in rank order. Users
/// missing from `membership` are treated as [`Membership::Unknown`] and
/// stay visible.
fn visible_users<'a>(
    board: &'a [(i64, i64)],
    membership: &HashMap<i64, Membership>,
    hide_quit_users: bool,
) -> Vec<&'a (i64, i64)> {
    board
        .iter()
        .filter(|(user_id, _)| {
            !hide_quit_users || membership.get(user_id) != Some(&Membership::Absent)
        })
        .collect()
}

/// The user ids that will actually be displayed for `requested_page`, after
/// filtering and clamping — at most [`PER_PAGE`] of them. Used to bound the
/// member-data lookups for name formats 1-3 to the displayed page only.
pub(crate) fn page_slice_ids(
    board: &[(i64, i64)],
    membership: &HashMap<i64, Membership>,
    hide_quit_users: bool,
    requested_page: i64,
) -> Vec<i64> {
    let visible = visible_users(board, membership, hide_quit_users);
    let bounds = clamp_page(requested_page, visible.len());
    visible
        .iter()
        .skip((bounds.page - 1) * PER_PAGE)
        .take(PER_PAGE)
        .map(|(user_id, _)| *user_id)
        .collect()
}

/// Pure view builder: filters, paginates (clamping first — F14), ranks and
/// formats. `board` is the full `(user_id, score)` list from
/// [`queries::leaderboard`], already sorted by score descending.
/// Users missing from `membership` are treated as [`Membership::Unknown`].
pub fn build_view(
    locale: &LanguageIdentifier,
    board: &[(i64, i64)],
    membership: &HashMap<i64, Membership>,
    hide_quit_users: bool,
    format: LeaderboardFormat,
    requested_page: i64,
) -> BoardView {
    let total_score: i64 = board.iter().map(|(_, score)| score).sum();

    let visible = visible_users(board, membership, hide_quit_users);

    let bounds = clamp_page(requested_page, visible.len());
    let start = (bounds.page - 1) * PER_PAGE;

    let rows = visible
        .iter()
        .skip(start)
        .take(PER_PAGE)
        .enumerate()
        .map(|(offset, (user_id, score))| {
            let rank = start + offset + 1;
            let name = display_name(
                format,
                *user_id,
                membership.get(user_id).unwrap_or(&Membership::Unknown),
            );
            i18n::t_args(
                locale,
                "leaderboard-row",
                &[
                    ("medal", medal(rank).into()),
                    ("rank", (rank as i64).into()),
                    ("name", name.into()),
                    ("score", (*score).into()),
                ],
            )
        })
        .collect();

    BoardView {
        total_score,
        rows,
        page: bounds.page,
        last: bounds.last,
    }
}

/// Builds the leaderboard embed from a computed view. The panel renderer
/// passes `with_footer = false` (legacy panels had no page footer).
pub fn leaderboard_embed(
    locale: &LanguageIdentifier,
    guild_name: &str,
    view: &BoardView,
    with_footer: bool,
) -> serenity::CreateEmbed {
    let rows = if view.rows.is_empty() {
        i18n::t(locale, "leaderboard-empty")
    } else {
        view.rows.join("\n")
    };

    let mut embed = serenity::CreateEmbed::new()
        .title(i18n::t_args(
            locale,
            "leaderboard-title",
            &[("guild", guild_name.to_string().into())],
        ))
        .description(i18n::t_args(
            locale,
            "leaderboard-total",
            &[("total", view.total_score.into())],
        ))
        .colour(util::COLOR_MAIN)
        .field(i18n::t(locale, "leaderboard-field-name"), rows, false);

    if with_footer {
        embed = embed.footer(serenity::CreateEmbedFooter::new(i18n::t_args(
            locale,
            "leaderboard-footer",
            &[
                ("page", (view.page as i64).into()),
                ("last", (view.last as i64).into()),
            ],
        )));
    }
    embed
}

/// True when the error means "this user is not a member of the guild".
fn is_unknown_member(error: &serenity::Error) -> bool {
    match error {
        serenity::Error::Http(serenity::HttpError::UnsuccessfulRequest(response)) => {
            response.status_code == serenity::StatusCode::NOT_FOUND
        }
        _ => false,
    }
}

/// A stored user id as a Discord snowflake, or `None` when it cannot be one
/// (zero or negative — such a user can never be a member).
fn snowflake(user_id: i64) -> Option<serenity::UserId> {
    u64::try_from(user_id)
        .ok()
        .filter(|id| *id != 0)
        .map(serenity::UserId::new)
}

/// Resolves ONE user's membership: Serenity checks the cache first and
/// falls back to a single HTTP fetch. 404 → [`Membership::Absent`]; any
/// other failure → [`Membership::Unknown`] (logged, F16).
pub async fn lookup_membership(
    cache_http: &impl serenity::CacheHttp,
    guild_id: serenity::GuildId,
    user_id: i64,
) -> Membership {
    let Some(id) = snowflake(user_id) else {
        return Membership::Absent;
    };
    match guild_id.member(cache_http, id).await {
        Ok(member) => Membership::Present {
            username: member.user.name.to_string(),
            nickname: member.nick.as_ref().map(|nick| nick.to_string()),
            tag: member.user.tag(),
        },
        Err(error) if is_unknown_member(&error) => Membership::Absent,
        Err(error) => {
            log::warn!(
                "Member lookup failed (guild={guild_id}, user={user_id}), keeping visible: {error}"
            );
            Membership::Unknown
        }
    }
}

/// Bounded membership walk for the quit-user filter: resolves `ids` in
/// order, stopping as soon as `needed_visible` non-absent users are
/// confirmed OR `budget` lookups have been spent. Users never reached stay
/// out of the map — [`build_view`] treats them as [`Membership::Unknown`]
/// (visible), so exhausting the budget can only ever SHOW a departed user,
/// never hide a present one. Invalid snowflakes are marked absent for free.
pub(crate) async fn resolve_until_visible<R, Fut>(
    ids: impl IntoIterator<Item = i64>,
    needed_visible: usize,
    budget: usize,
    mut resolve: R,
) -> HashMap<i64, Membership>
where
    R: FnMut(i64) -> Fut,
    Fut: std::future::Future<Output = Membership>,
{
    let mut resolved = HashMap::new();
    let mut visible = 0usize;
    let mut lookups = 0usize;
    for user_id in ids {
        if visible >= needed_visible {
            break;
        }
        if resolved.contains_key(&user_id) {
            continue;
        }
        let membership = if snowflake(user_id).is_none() {
            // Not a valid snowflake — cannot be a member; no lookup spent.
            Membership::Absent
        } else {
            if lookups >= budget {
                log::debug!(
                    "Membership lookup budget ({budget}) exhausted after {visible} visible users; \
                     remaining board users stay visible"
                );
                break;
            }
            lookups += 1;
            resolve(user_id).await
        };
        if membership != Membership::Absent {
            visible += 1;
        }
        resolved.insert(user_id, membership);
    }
    resolved
}

/// How many visible users must be confirmed to render `requested_page`: a
/// full page's worth up to that page, clamped by the (unfiltered) board
/// length — filtering can only shrink the real last page below this bound.
pub(crate) fn needed_visible(requested_page: i64, board_len: usize) -> usize {
    let max_last = board_len.div_ceil(PER_PAGE).max(1);
    let page = requested_page.clamp(1, max_last as i64) as usize;
    (page * PER_PAGE).min(board_len)
}

/// End-to-end render: queries the board and settings, resolves only the
/// memberships actually needed — bounded to [`MEMBERSHIP_LOOKUP_BUDGET`]
/// filter lookups plus at most [`PER_PAGE`] name lookups — and returns the
/// finished embed. Both `/leaderboard` and the panel updater go through
/// here (shared path, F13).
pub async fn render_leaderboard(
    db: &impl ConnectionTrait,
    cache_http: &impl serenity::CacheHttp,
    guild_id: serenity::GuildId,
    guild_name: &str,
    requested_page: i64,
    locale: &LanguageIdentifier,
    with_footer: bool,
) -> Result<serenity::CreateEmbed, crate::bot::Error> {
    let db_guild_id = i64::try_from(guild_id.get())?;
    let board = queries::leaderboard(db, db_guild_id).await?;
    let effective = settings::effective_settings(db, db_guild_id).await?;

    let hide_quit_users = effective.hide_quit_users == 0;
    let format = LeaderboardFormat::from_setting(effective.leaderboard_format);

    // Quit-user filter: walk the board in rank order, stopping once the
    // requested page can be filled — 10 lookups on a healthy board — and
    // never spending more than the hard budget (unresolved users default
    // to Unknown = visible).
    let mut membership = if hide_quit_users {
        let ids: Vec<i64> = board.iter().map(|entry| entry.0).collect();
        resolve_until_visible(
            ids,
            needed_visible(requested_page, board.len()),
            MEMBERSHIP_LOOKUP_BUDGET,
            |user_id| lookup_membership(cache_http, guild_id, user_id),
        )
        .await
    } else {
        HashMap::new()
    };

    // Name formats 1-3 need member data for the displayed page only (≤ 10
    // extra lookups); anything still unresolved renders as a mention (F13).
    if format != LeaderboardFormat::Mention {
        for user_id in page_slice_ids(&board, &membership, hide_quit_users, requested_page) {
            if !membership.contains_key(&user_id) {
                let resolved = lookup_membership(cache_http, guild_id, user_id).await;
                membership.insert(user_id, resolved);
            }
        }
    }
    let view = build_view(
        locale,
        &board,
        &membership,
        hide_quit_users,
        format,
        requested_page,
    );
    Ok(leaderboard_embed(locale, guild_name, &view, with_footer))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn locale() -> LanguageIdentifier {
        i18n::resolve(None)
    }

    fn present(username: &str, nickname: Option<&str>, tag: &str) -> Membership {
        Membership::Present {
            username: username.to_string(),
            nickname: nickname.map(str::to_string),
            tag: tag.to_string(),
        }
    }

    // --- F14: page clamping ---

    #[test]
    fn clamp_page_zero_and_negative_become_first_page() {
        assert_eq!(clamp_page(0, 25), PageBounds { page: 1, last: 3 });
        assert_eq!(clamp_page(-3, 25), PageBounds { page: 1, last: 3 });
    }

    #[test]
    fn clamp_page_past_the_end_becomes_last_page() {
        assert_eq!(clamp_page(999, 15), PageBounds { page: 2, last: 2 });
    }

    #[test]
    fn clamp_page_exact_boundaries() {
        assert_eq!(clamp_page(1, 10), PageBounds { page: 1, last: 1 });
        assert_eq!(clamp_page(2, 11), PageBounds { page: 2, last: 2 });
    }

    #[test]
    fn clamp_page_empty_list_is_a_single_page() {
        assert_eq!(clamp_page(1, 0), PageBounds { page: 1, last: 1 });
        assert_eq!(clamp_page(42, 0), PageBounds { page: 1, last: 1 });
    }

    // --- medals ---

    #[test]
    fn medals_for_top_three_then_generic() {
        assert_eq!(medal(1), "🥇");
        assert_eq!(medal(2), "🥈");
        assert_eq!(medal(3), "🥉");
        assert_eq!(medal(4), ":medal:");
        assert_eq!(medal(100), ":medal:");
    }

    // --- format parsing ---

    #[test]
    fn format_from_setting_with_graceful_fallback() {
        assert_eq!(LeaderboardFormat::from_setting(0), LeaderboardFormat::Mention);
        assert_eq!(LeaderboardFormat::from_setting(1), LeaderboardFormat::Username);
        assert_eq!(LeaderboardFormat::from_setting(2), LeaderboardFormat::Nickname);
        assert_eq!(LeaderboardFormat::from_setting(3), LeaderboardFormat::UsernameTag);
        assert_eq!(LeaderboardFormat::from_setting(7), LeaderboardFormat::Mention);
        assert_eq!(LeaderboardFormat::from_setting(-1), LeaderboardFormat::Mention);
    }

    // --- F13: name rendering never depends on a successful fetch ---

    #[test]
    fn mention_format_needs_no_member_data() {
        assert_eq!(
            display_name(LeaderboardFormat::Mention, 42, &Membership::Absent),
            "<@42>"
        );
        assert_eq!(
            display_name(
                LeaderboardFormat::Mention,
                42,
                &present("ana", Some("Annie"), "ana#0")
            ),
            "<@42>"
        );
    }

    #[test]
    fn username_nickname_and_tag_formats_use_member_data() {
        let member = present("ana", Some("Annie"), "ana#0");
        assert_eq!(display_name(LeaderboardFormat::Username, 42, &member), "ana");
        assert_eq!(display_name(LeaderboardFormat::Nickname, 42, &member), "Annie");
        assert_eq!(display_name(LeaderboardFormat::UsernameTag, 42, &member), "ana#0");
    }

    #[test]
    fn nickname_format_falls_back_to_username() {
        let member = present("ana", None, "ana#0");
        assert_eq!(display_name(LeaderboardFormat::Nickname, 42, &member), "ana");
    }

    #[test]
    fn non_mention_formats_fall_back_to_mention_for_absent_or_unknown_users() {
        for format in [
            LeaderboardFormat::Username,
            LeaderboardFormat::Nickname,
            LeaderboardFormat::UsernameTag,
        ] {
            assert_eq!(display_name(format, 42, &Membership::Absent), "<@42>");
            assert_eq!(display_name(format, 42, &Membership::Unknown), "<@42>");
        }
    }

    // --- bounded membership resolution (no unbounded per-user HTTP) ---

    fn ready(membership: Membership) -> std::future::Ready<Membership> {
        std::future::ready(membership)
    }

    #[tokio::test]
    async fn filter_walk_stops_once_the_requested_page_is_filled() {
        // 100-user board, page 1, everyone still a member: exactly 10
        // lookups (spec target: "HTTP fallback per page (only 10 lookups)").
        let ids: Vec<i64> = (1..=100).collect();
        let calls = std::cell::Cell::new(0usize);
        let map = resolve_until_visible(ids.iter().copied(), 10, MEMBERSHIP_LOOKUP_BUDGET, |_| {
            calls.set(calls.get() + 1);
            ready(present("u", None, "u#0"))
        })
        .await;
        assert_eq!(calls.get(), 10);
        assert_eq!(map.len(), 10);
    }

    #[tokio::test]
    async fn departed_users_extend_the_walk_only_up_to_the_hard_budget() {
        // Pathological board where everyone left: the page can never fill,
        // so the hard budget stops the walk; the rest stay unresolved
        // (Unknown → visible), never triggering more HTTP.
        let ids: Vec<i64> = (1..=10_000).collect();
        let calls = std::cell::Cell::new(0usize);
        let map = resolve_until_visible(ids.iter().copied(), 10, MEMBERSHIP_LOOKUP_BUDGET, |_| {
            calls.set(calls.get() + 1);
            ready(Membership::Absent)
        })
        .await;
        assert_eq!(calls.get(), MEMBERSHIP_LOOKUP_BUDGET);
        assert_eq!(map.len(), MEMBERSHIP_LOOKUP_BUDGET);
        assert!(!map.contains_key(&((MEMBERSHIP_LOOKUP_BUDGET as i64) + 1)));
    }

    #[tokio::test]
    async fn filter_walk_counts_only_visible_users_toward_the_page() {
        // Alternating absent/present: confirming 5 visible users takes 10
        // lookups, and absent users are correctly recorded along the way.
        let ids: Vec<i64> = (1..=100).collect();
        let calls = std::cell::Cell::new(0usize);
        let map = resolve_until_visible(ids.iter().copied(), 5, MEMBERSHIP_LOOKUP_BUDGET, |id| {
            calls.set(calls.get() + 1);
            ready(if id % 2 == 1 {
                Membership::Absent
            } else {
                Membership::Unknown
            })
        })
        .await;
        assert_eq!(calls.get(), 10);
        assert_eq!(map.get(&1), Some(&Membership::Absent));
        assert_eq!(map.get(&10), Some(&Membership::Unknown));
        assert!(!map.contains_key(&11));
    }

    #[tokio::test]
    async fn invalid_snowflakes_are_absent_without_spending_budget() {
        let calls = std::cell::Cell::new(0usize);
        let map = resolve_until_visible([0, -5, 7], 10, MEMBERSHIP_LOOKUP_BUDGET, |id| {
            calls.set(calls.get() + 1);
            assert_eq!(id, 7, "only the real snowflake reaches the resolver");
            ready(present("u", None, "u#0"))
        })
        .await;
        assert_eq!(calls.get(), 1);
        assert_eq!(map.get(&0), Some(&Membership::Absent));
        assert_eq!(map.get(&-5), Some(&Membership::Absent));
    }

    #[test]
    fn needed_visible_covers_the_clamped_requested_page() {
        assert_eq!(needed_visible(1, 100), 10);
        assert_eq!(needed_visible(3, 100), 30);
        assert_eq!(needed_visible(-2, 100), 10, "bad pages clamp to page 1");
        assert_eq!(needed_visible(999, 25), 25, "page clamps to 3, capped by board length");
        assert_eq!(needed_visible(1, 0), 0);
        assert_eq!(needed_visible(1, 7), 7);
    }

    #[test]
    fn page_slice_ids_follow_filtering_and_clamping() {
        let board: Vec<(i64, i64)> = (1..=12).map(|i| (i, 50 - i)).collect();
        let membership = HashMap::from([(1, Membership::Absent)]);
        // Hiding quit users: visible = 2..=12 (11 users), page 2 = [12].
        assert_eq!(page_slice_ids(&board, &membership, true, 2), vec![12]);
        // Showing quit users: page 2 = [11, 12].
        assert_eq!(page_slice_ids(&board, &membership, false, 2), vec![11, 12]);
        // Out-of-range pages clamp (F14) before slicing.
        assert_eq!(page_slice_ids(&board, &HashMap::new(), false, 999), vec![11, 12]);
        assert_eq!(page_slice_ids(&board, &HashMap::new(), false, -1).len(), PER_PAGE);
    }

    // --- build_view ---

    #[test]
    fn total_is_the_whole_server_score_independent_of_filters() {
        // User 2 is absent (hidden) and user 3 has score 0 — both still
        // count toward the total (F15).
        let board = vec![(1, 10), (2, 5), (3, 0)];
        let membership = HashMap::from([
            (1, present("a", None, "a#0")),
            (2, Membership::Absent),
            (3, present("c", None, "c#0")),
        ]);
        let view = build_view(
            &locale(),
            &board,
            &membership,
            true,
            LeaderboardFormat::Mention,
            1,
        );
        assert_eq!(view.total_score, 15);
        assert_eq!(view.rows.len(), 2, "absent user hidden from rows");
    }

    #[test]
    fn zero_score_users_with_awards_are_listed() {
        let board = vec![(1, 5), (2, 0)];
        let view = build_view(
            &locale(),
            &board,
            &HashMap::new(),
            false,
            LeaderboardFormat::Mention,
            1,
        );
        assert_eq!(view.rows.len(), 2);
        assert!(view.rows[1].contains("<@2>"), "got: {}", view.rows[1]);
        assert!(view.rows[1].contains('0'), "got: {}", view.rows[1]);
    }

    #[test]
    fn hide_quit_users_drops_absent_but_keeps_unknown_and_unresolved() {
        let board = vec![(1, 30), (2, 20), (3, 10)];
        // 1 absent, 2 unknown (failed lookup), 3 not resolved at all.
        let membership = HashMap::from([(1, Membership::Absent), (2, Membership::Unknown)]);
        let view = build_view(
            &locale(),
            &board,
            &membership,
            true,
            LeaderboardFormat::Mention,
            1,
        );
        assert_eq!(view.rows.len(), 2);
        assert!(view.rows[0].contains("<@2>"), "got: {}", view.rows[0]);
        assert!(view.rows[1].contains("<@3>"), "got: {}", view.rows[1]);
    }

    #[test]
    fn show_quit_users_keeps_absent_users_with_mention_fallback_names() {
        let board = vec![(1, 30)];
        let membership = HashMap::from([(1, Membership::Absent)]);
        let view = build_view(
            &locale(),
            &board,
            &membership,
            false, // hide_quit_users setting = 1 (show)
            LeaderboardFormat::Username,
            1,
        );
        assert_eq!(view.rows.len(), 1);
        assert!(
            view.rows[0].contains("<@1>"),
            "departed user must render as mention, not crash: {}",
            view.rows[0]
        );
    }

    #[test]
    fn ranks_and_medals_follow_the_clamped_page() {
        // 15 users → 2 pages. Requesting page 999 must show page 2 with
        // ranks 11-15 and NO medals (F14 — legacy showed ranks 9981+).
        let board: Vec<(i64, i64)> = (1..=15).map(|i| (i, 100 - i)).collect();
        let view = build_view(
            &locale(),
            &board,
            &HashMap::new(),
            false,
            LeaderboardFormat::Mention,
            999,
        );
        assert_eq!((view.page, view.last), (2, 2));
        assert_eq!(view.rows.len(), 5);
        assert!(view.rows[0].contains("11"), "got: {}", view.rows[0]);
        assert!(view.rows[4].contains("15"), "got: {}", view.rows[4]);
        assert!(view.rows.iter().all(|row| !row.contains('🥇')));

        // A negative page clamps to 1 and keeps the medals.
        let view = build_view(
            &locale(),
            &board,
            &HashMap::new(),
            false,
            LeaderboardFormat::Mention,
            -3,
        );
        assert_eq!((view.page, view.last), (1, 2));
        assert_eq!(view.rows.len(), PER_PAGE);
        assert!(view.rows[0].contains('🥇'), "got: {}", view.rows[0]);
        assert!(view.rows[1].contains('🥈'), "got: {}", view.rows[1]);
        assert!(view.rows[2].contains('🥉'), "got: {}", view.rows[2]);
        assert!(view.rows[3].contains(":medal:"), "got: {}", view.rows[3]);
    }

    #[test]
    fn ranks_are_positions_in_the_visible_list_when_filtering() {
        // With user 1 hidden, user 2 becomes visible rank 1 and gets 🥇.
        let board = vec![(1, 30), (2, 20)];
        let membership = HashMap::from([(1, Membership::Absent)]);
        let view = build_view(
            &locale(),
            &board,
            &membership,
            true,
            LeaderboardFormat::Mention,
            1,
        );
        assert_eq!(view.rows.len(), 1);
        assert!(view.rows[0].contains('🥇'), "got: {}", view.rows[0]);
        assert!(view.rows[0].contains("<@2>"), "got: {}", view.rows[0]);
    }

    #[test]
    fn empty_board_yields_no_rows_and_single_page() {
        let view = build_view(
            &locale(),
            &[],
            &HashMap::new(),
            true,
            LeaderboardFormat::Mention,
            1,
        );
        assert_eq!(view.rows.len(), 0);
        assert_eq!(view.total_score, 0);
        assert_eq!((view.page, view.last), (1, 1));
    }

    // --- embed assembly ---

    fn embed_json(embed: &serenity::CreateEmbed) -> serde_json::Value {
        serde_json::to_value(embed).expect("serialize embed")
    }

    #[test]
    fn embed_contains_title_total_rows_and_footer() {
        let board = vec![(1, 10), (2, 5)];
        let view = build_view(
            &locale(),
            &board,
            &HashMap::new(),
            false,
            LeaderboardFormat::Mention,
            1,
        );
        let embed = leaderboard_embed(&locale(), "My Server", &view, true);
        let json = embed_json(&embed);

        let title = json["title"].as_str().unwrap();
        assert!(title.contains("My Server"), "got: {title}");
        assert!(title.contains('🏆'), "got: {title}");
        let description = json["description"].as_str().unwrap();
        assert!(description.contains("15"), "got: {description}");
        let field = json["fields"][0]["value"].as_str().unwrap();
        assert!(field.contains("<@1>") && field.contains("<@2>"), "got: {field}");
        let footer = json["footer"]["text"].as_str().unwrap();
        assert!(footer.contains('1'), "got: {footer}");
    }

    #[test]
    fn embed_without_footer_for_panels_and_empty_placeholder() {
        let view = build_view(
            &locale(),
            &[],
            &HashMap::new(),
            true,
            LeaderboardFormat::Mention,
            1,
        );
        let embed = leaderboard_embed(&locale(), "My Server", &view, false);
        let json = embed_json(&embed);
        assert!(json.get("footer").is_none() || json["footer"].is_null());
        let field = json["fields"][0]["value"].as_str().unwrap();
        assert_eq!(field, i18n::t(&locale(), "leaderboard-empty"));
        assert_ne!(field, "leaderboard-empty", "catalog key must exist");
    }

    #[test]
    fn catalog_keys_exist() {
        let locale = locale();
        for key in [
            "leaderboard-empty",
            "leaderboard-field-name",
            "leaderboard-guild-fallback",
        ] {
            assert_ne!(i18n::t(&locale, key), key, "missing catalog key {key}");
        }
        let row = i18n::t_args(
            &locale,
            "leaderboard-row",
            &[
                ("medal", "🥇".into()),
                ("rank", 1.into()),
                ("name", "<@42>".into()),
                ("score", 10.into()),
            ],
        );
        assert!(row.contains("🥇") && row.contains("<@42>") && row.contains("10"), "got: {row}");
    }

    // --- integration: DB query → view (sqlite::memory:) ---

    mod db {
        use super::*;
        use sea_orm::{ActiveModelTrait, DatabaseConnection, Set};
        use uuid::Uuid;

        use crate::domain::test_support::{fresh_db, insert_guild, now};
        use crate::entities::{trophies, user_trophies};

        async fn insert_trophy(
            db: &DatabaseConnection,
            guild_id: i64,
            name: &str,
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
                emoji: Set("🏆".to_string()),
                value: Set(value),
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
        async fn query_plus_view_lists_zero_score_users_and_real_total() {
            let db = fresh_db().await;
            insert_guild(&db, 1).await;
            let plus = insert_trophy(&db, 1, "Plus", 5).await;
            let minus = insert_trophy(&db, 1, "Minus", -5).await;

            award(&db, 1, 10, plus).await; // score 5
            award(&db, 1, 20, plus).await;
            award(&db, 1, 20, minus).await; // score 0 — must still appear (F15)

            let board = queries::leaderboard(&db, 1).await.expect("query board");
            let membership = HashMap::from([(20, Membership::Absent)]);
            let view = build_view(
                &locale(),
                &board,
                &membership,
                true,
                LeaderboardFormat::Mention,
                1,
            );

            // User 20 is hidden but the total stays the real aggregate.
            assert_eq!(view.total_score, 5);
            assert_eq!(view.rows.len(), 1);
            assert!(view.rows[0].contains("<@10>"), "got: {}", view.rows[0]);

            // With "Show Quit Users", the zero-score departed user is listed.
            let view = build_view(
                &locale(),
                &board,
                &membership,
                false,
                LeaderboardFormat::Mention,
                1,
            );
            assert_eq!(view.rows.len(), 2);
            assert!(view.rows[1].contains("<@20>"), "got: {}", view.rows[1]);
        }
    }
}
