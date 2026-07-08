//! Discord-side reward application — the §2 reward engine (born in batch C3,
//! reused by `/award`, `/revoke` and `/clear`).
//!
//! Replaces the legacy `doRewardRoles` (dead under discord.js v14 except in
//! Administrator guilds, where its unhandled rejections crashed the process):
//! - the user's score is RECOMPUTED from the database (ADR 0006), never read
//!   from a stored counter;
//! - the target role set comes from the pure `domain::rewards::target_roles`
//!   plus the guild's `stack_roles` setting — ONE final set per user, so the
//!   legacy add-then-remove ordering hazard (F22) cannot occur;
//! - the diff against the member's current roles only ever touches configured
//!   reward roles — unrelated roles are never removed;
//! - every Discord call is AWAITED; hierarchy violations, managed roles and
//!   deleted roles are skipped with a log line; API failures are logged with
//!   full context. Nothing here panics or takes the process down;
//! - applications for the SAME (guild, user) are serialized through a
//!   per-pair async lock, and the score is recomputed under that lock — so
//!   concurrent `/award` + `/revoke` for one member cannot interleave into a
//!   role set that no longer matches the final committed score (TOCTOU).
//!   The last invocation to run always recomputes from the fully committed
//!   database state and converges the member onto it. Distinct (guild, user)
//!   pairs never block each other.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, LazyLock, Mutex as StdMutex, Weak};

use poise::serenity_prelude as serenity;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};

use crate::bot::Context;
use crate::domain::rewards::{target_roles, StackMode};
use crate::domain::settings::{get_setting, Setting};
use crate::domain::queries;
use crate::entities::role_rewards;

/// The role additions and removals needed to move a member onto the target
/// reward set. Only configured reward roles ever appear in `remove`.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct RolePlan {
    pub add: Vec<i64>,
    pub remove: Vec<i64>,
}

impl RolePlan {
    pub fn is_empty(&self) -> bool {
        self.add.is_empty() && self.remove.is_empty()
    }
}

/// What the engine needs to know about a guild role to decide assignability.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RoleMeta {
    /// Position in the guild role hierarchy (0 = @everyone).
    pub position: u16,
    /// Integration/bot-managed roles can never be assigned manually.
    pub managed: bool,
}

/// Per-(guild, user) apply locks. Entries are weak so the map never grows
/// beyond the pairs currently being applied; dead entries are pruned on every
/// lookup. A `std` mutex guards the map itself (held only for the lookup,
/// never across an await); the returned `tokio` mutex is what serializes the
/// recompute-then-apply critical section.
type ApplyLockMap = HashMap<(i64, i64), Weak<tokio::sync::Mutex<()>>>;

static APPLY_LOCKS: LazyLock<StdMutex<ApplyLockMap>> =
    LazyLock::new(|| StdMutex::new(HashMap::new()));

/// The serialization lock for one (guild, user) pair. Two callers passing the
/// same pair get the same lock; distinct pairs get independent locks.
fn apply_lock(guild_id: i64, user_id: i64) -> Arc<tokio::sync::Mutex<()>> {
    let mut map = APPLY_LOCKS.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    map.retain(|_, weak| weak.strong_count() > 0);
    if let Some(existing) = map.get(&(guild_id, user_id)).and_then(Weak::upgrade) {
        return existing;
    }
    let lock = Arc::new(tokio::sync::Mutex::new(()));
    map.insert((guild_id, user_id), Arc::downgrade(&lock));
    lock
}

/// Pure diff: which reward roles to add and which to remove so the member
/// ends up holding exactly `target` (idempotent — a member already on target
/// yields an empty plan). `configured` is every reward role of the guild;
/// roles outside it are never touched. Order follows the input slices and
/// duplicates are collapsed.
pub fn plan_changes(member_roles: &[i64], configured: &[i64], target: &[i64]) -> RolePlan {
    let member: HashSet<i64> = member_roles.iter().copied().collect();
    let target_set: HashSet<i64> = target.iter().copied().collect();
    let configured_set: HashSet<i64> = configured.iter().copied().collect();

    let mut seen = HashSet::new();
    let add = target
        .iter()
        .copied()
        .filter(|id| !member.contains(id) && seen.insert(*id))
        .collect();

    let mut seen = HashSet::new();
    let remove = member_roles
        .iter()
        .copied()
        .filter(|id| {
            configured_set.contains(id) && !target_set.contains(id) && seen.insert(*id)
        })
        .collect();

    RolePlan { add, remove }
}

/// Highest role position the bot holds (0 when it only has @everyone or its
/// roles are unknown). Roles missing from `roles` are ignored.
pub fn bot_top_position(bot_roles: &[i64], roles: &HashMap<i64, RoleMeta>) -> u16 {
    bot_roles
        .iter()
        .filter_map(|id| roles.get(id))
        .map(|meta| meta.position)
        .max()
        .unwrap_or(0)
}

/// Splits candidate role ids into `(assignable, skipped)`. A role is
/// assignable when it still exists in the guild, is not integration-managed,
/// and sits STRICTLY below the bot's highest role (equal position is not
/// manageable on Discord).
pub fn filter_assignable(
    candidates: &[i64],
    roles: &HashMap<i64, RoleMeta>,
    bot_top: u16,
) -> (Vec<i64>, Vec<i64>) {
    candidates.iter().copied().partition(|id| {
        roles
            .get(id)
            .is_some_and(|meta| !meta.managed && meta.position < bot_top)
    })
}

/// DB half of the engine, kept `ConnectionTrait`-generic for tests: the
/// user's recomputed score is turned into `(target, configured)` role-id
/// sets. `None` when the guild has no reward roles configured (nothing to
/// apply, nothing to remove).
pub async fn target_for_user(
    db: &impl sea_orm::ConnectionTrait,
    guild_id: i64,
    user_id: i64,
) -> anyhow::Result<Option<(Vec<i64>, Vec<i64>)>> {
    let rewards: Vec<(i64, i64)> = role_rewards::Entity::find()
        .filter(role_rewards::Column::GuildId.eq(guild_id))
        .all(db)
        .await?
        .into_iter()
        .map(|row| (row.role_id, i64::from(row.requirement)))
        .collect();
    if rewards.is_empty() {
        return Ok(None);
    }

    let score = queries::user_score(db, guild_id, user_id).await?;
    let stack = StackMode::from_setting(get_setting(db, guild_id, Setting::StackRoles).await?);
    let target = target_roles(score, &rewards, stack);
    let configured = rewards.into_iter().map(|(role_id, _)| role_id).collect();
    Ok(Some((target, configured)))
}

/// Snapshot of a guild's role metadata as the engine consumes it.
fn role_meta_map<'a>(
    roles: impl IntoIterator<Item = (&'a serenity::RoleId, &'a serenity::Role)>,
) -> HashMap<i64, RoleMeta> {
    roles
        .into_iter()
        .map(|(id, role)| {
            (id.get() as i64, RoleMeta { position: role.position, managed: role.managed })
        })
        .collect()
}

/// Recomputes and applies the reward roles for one user in one guild. Called
/// after `/award`, `/revoke` and `/clear` commit their database changes.
///
/// Thin interaction-context wrapper over [`apply_rewards_via`]: it snapshots
/// the gateway cache's role map (when the guild is cached) so the shared
/// engine skips the HTTP role fetch, exactly as before.
pub async fn apply_rewards(
    ctx: &Context<'_>,
    guild_id: serenity::GuildId,
    user_id: serenity::UserId,
) -> anyhow::Result<()> {
    let cached_roles = ctx.guild().map(|guild| role_meta_map(guild.roles.iter()));
    let bot_id = ctx.serenity_context().cache.current_user().id;
    apply_rewards_via(
        &ctx.data().db,
        ctx.serenity_context(),
        bot_id,
        cached_roles,
        guild_id,
        user_id,
    )
    .await
}

/// The reward engine proper, callable from any [`serenity::CacheHttp`]
/// (interaction contexts AND cache-less HTTP clients like the smoke harness).
/// `cached_roles` lets callers inject an already-known role map; `None`
/// fetches the guild roles over HTTP.
///
/// Behavior guarantees (§2 of the parity plan):
/// - non-members are a logged no-op (legacy /award allows awarding them);
/// - roles above/at the bot's highest role, managed roles and roles deleted
///   from the guild (F24 keeps their rows) are skipped with a warning;
/// - each add/remove is awaited individually — one failing role does not
///   abort the others, every failure is logged with guild/user/role context;
/// - the whole recompute→diff→apply section runs under a per-(guild, user)
///   lock, so concurrent score-changing commands for the same member cannot
///   race each other into stale role state (the score is re-read from the
///   database only after the lock is held).
///
/// `Err` is returned only for infrastructure failures before any role change
/// is attempted (DB errors, role list unavailable); callers log it — the
/// triggering command has already committed and replied.
pub async fn apply_rewards_via(
    db: &sea_orm::DatabaseConnection,
    cache_http: &impl serenity::CacheHttp,
    bot_id: serenity::UserId,
    cached_roles: Option<HashMap<i64, RoleMeta>>,
    guild_id: serenity::GuildId,
    user_id: serenity::UserId,
) -> anyhow::Result<()> {
    let gid = guild_id.get() as i64;
    let uid = user_id.get() as i64;

    // Serialize with any other in-flight application for this member. The
    // score/target recompute below MUST happen after the lock is acquired:
    // that is what closes the TOCTOU between a command's commit and its role
    // application when /award and /revoke race for the same user.
    let lock = apply_lock(gid, uid);
    let _guard = lock.lock().await;

    let Some((target, configured)) = target_for_user(db, gid, uid).await? else {
        return Ok(());
    };

    let member = match guild_id.member(cache_http, user_id).await {
        Ok(member) => member,
        Err(err) => {
            log::debug!(
                "reward target user {uid} is not a member of guild {gid}, skipping role application: {err}"
            );
            return Ok(());
        }
    };
    let member_roles: Vec<i64> = member.roles.iter().map(|id| id.get() as i64).collect();

    let plan = plan_changes(&member_roles, &configured, &target);
    if plan.is_empty() {
        return Ok(());
    }

    let roles = match cached_roles {
        Some(roles) => roles,
        None => role_meta_map(&guild_id.roles(cache_http.http()).await?),
    };
    let bot_top = match guild_id.member(cache_http, bot_id).await {
        Ok(bot_member) => {
            let bot_roles: Vec<i64> = bot_member.roles.iter().map(|id| id.get() as i64).collect();
            bot_top_position(&bot_roles, &roles)
        }
        Err(err) => {
            log::warn!("could not resolve the bot's own member in guild {gid}: {err}");
            0
        }
    };

    let (add, skipped_add) = filter_assignable(&plan.add, &roles, bot_top);
    let (remove, skipped_remove) = filter_assignable(&plan.remove, &roles, bot_top);
    for role_id in skipped_add.iter().chain(&skipped_remove) {
        log::warn!(
            "skipping reward role {role_id} in guild {gid}: deleted, managed, or not below the bot's highest role"
        );
    }

    for role_id in add {
        if let Err(err) = member
            .add_role(cache_http.http(), serenity::RoleId::new(role_id as u64))
            .await
        {
            log::error!("failed to add reward role {role_id} to user {uid} in guild {gid}: {err}");
        }
    }
    for role_id in remove {
        if let Err(err) = member
            .remove_role(cache_http.http(), serenity::RoleId::new(role_id as u64))
            .await
        {
            log::error!(
                "failed to remove reward role {role_id} from user {uid} in guild {gid}: {err}"
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sea_orm::{ActiveModelTrait, DatabaseConnection, Set};
    use uuid::Uuid;

    use crate::domain::normalize::normalize_name;
    use crate::domain::test_support::{fresh_db, insert_guild, now};
    use crate::entities::{trophies, user_trophies};

    fn meta(position: u16) -> RoleMeta {
        RoleMeta { position, managed: false }
    }

    // --- apply_lock (per-(guild, user) serialization, TOCTOU fix) ---
    //
    // These tests use guild ids in the 900_000+ range, unique per test, so
    // parallel tests never contend on the process-wide APPLY_LOCKS registry.

    #[tokio::test]
    async fn same_pair_shares_one_lock_and_distinct_pairs_do_not() {
        let a1 = apply_lock(900_001, 1);
        let a2 = apply_lock(900_001, 1);
        let other_user = apply_lock(900_001, 2);
        let other_guild = apply_lock(900_002, 1);
        assert!(Arc::ptr_eq(&a1, &a2), "same (guild, user) must share one lock");
        assert!(!Arc::ptr_eq(&a1, &other_user), "different users must not share a lock");
        assert!(!Arc::ptr_eq(&a1, &other_guild), "different guilds must not share a lock");
    }

    #[tokio::test]
    async fn lock_registry_prunes_dead_entries() {
        let first = apply_lock(900_010, 7);
        let weak = Arc::downgrade(&first);
        drop(first);
        // A fresh lookup prunes the dead entry and mints a new lock.
        let second = apply_lock(900_010, 7);
        assert!(weak.upgrade().is_none(), "dropped lock must not be resurrected");
        let map = APPLY_LOCKS.lock().unwrap();
        assert!(
            map.get(&(900_010, 7)).is_some_and(|w| w.strong_count() == 1),
            "registry must hold exactly the live lock"
        );
        drop(map);
        drop(second);
    }

    #[tokio::test]
    async fn concurrent_applications_for_one_member_are_serialized() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let in_section = Arc::new(AtomicUsize::new(0));
        let overlaps = Arc::new(AtomicUsize::new(0));
        let mut handles = Vec::new();
        for _ in 0..8 {
            let in_section = Arc::clone(&in_section);
            let overlaps = Arc::clone(&overlaps);
            handles.push(tokio::spawn(async move {
                let lock = apply_lock(900_020, 42);
                let _guard = lock.lock().await;
                if in_section.fetch_add(1, Ordering::SeqCst) != 0 {
                    overlaps.fetch_add(1, Ordering::SeqCst);
                }
                // Yield inside the critical section: without the lock another
                // task would enter here and the overlap counter would trip.
                tokio::task::yield_now().await;
                in_section.fetch_sub(1, Ordering::SeqCst);
            }));
        }
        for handle in handles {
            handle.await.expect("task");
        }
        assert_eq!(overlaps.load(Ordering::SeqCst), 0, "critical sections overlapped");
    }

    /// The confirmed TOCTOU scenario: `/award` +100 and `/revoke` -100 race
    /// for the same member (score 0, reward role requires 100). Each task
    /// "commits" its score change first, then — under the apply lock —
    /// re-reads the score and rewrites the role state, with a yield inside
    /// the window that previously let a stale target land last. Serialized
    /// recompute-under-lock means whichever application runs last has seen
    /// every commit whose application already ran, so the final role state
    /// always matches the final score (0 → no role), in every interleaving.
    #[tokio::test]
    async fn racing_award_and_revoke_converge_on_the_final_score() {
        use std::sync::atomic::{AtomicI64, Ordering};

        for _ in 0..50 {
            let score = Arc::new(AtomicI64::new(0));
            let has_role = Arc::new(std::sync::Mutex::new(false));
            let mut handles = Vec::new();
            for delta in [100i64, -100] {
                let score = Arc::clone(&score);
                let has_role = Arc::clone(&has_role);
                handles.push(tokio::spawn(async move {
                    // The command's own DB commit happens before the engine runs.
                    score.fetch_add(delta, Ordering::SeqCst);
                    let lock = apply_lock(900_030, 9);
                    let _guard = lock.lock().await;
                    // Recompute under the lock (mirrors target_for_user)...
                    let current = score.load(Ordering::SeqCst);
                    // ...yield inside the old race window...
                    tokio::task::yield_now().await;
                    // ...then apply the roles for the recomputed score.
                    *has_role.lock().unwrap() = current >= 100;
                }));
            }
            for handle in handles {
                handle.await.expect("task");
            }
            assert!(
                !*has_role.lock().unwrap(),
                "final score is 0: the member must not keep the reward role"
            );
        }
    }

    // --- plan_changes ---

    #[test]
    fn member_already_on_target_yields_empty_plan() {
        let plan = plan_changes(&[10, 20, 99], &[10, 20, 30], &[10, 20]);
        assert!(plan.is_empty(), "idempotent: {plan:?}");
    }

    #[test]
    fn adds_missing_target_roles_and_removes_stale_reward_roles() {
        // Member holds reward role 10 (stale) and unrelated 99; target is 30.
        let plan = plan_changes(&[10, 99], &[10, 20, 30], &[30]);
        assert_eq!(plan.add, vec![30]);
        assert_eq!(plan.remove, vec![10]);
    }

    #[test]
    fn never_removes_roles_outside_the_configured_reward_set() {
        let plan = plan_changes(&[99, 42], &[10], &[]);
        assert!(plan.remove.is_empty(), "unrelated roles must not be touched: {plan:?}");
    }

    #[test]
    fn empty_target_removes_all_held_reward_roles() {
        let plan = plan_changes(&[10, 20, 99], &[10, 20, 30], &[]);
        assert!(plan.add.is_empty());
        assert_eq!(plan.remove, vec![10, 20]);
    }

    #[test]
    fn duplicate_inputs_are_collapsed() {
        let plan = plan_changes(&[10, 10], &[10, 20, 20], &[20, 20]);
        assert_eq!(plan.add, vec![20]);
        assert_eq!(plan.remove, vec![10]);
    }

    // --- hierarchy filtering ---

    #[test]
    fn bot_top_position_is_highest_known_role_defaulting_to_zero() {
        let roles = HashMap::from([(1, meta(3)), (2, meta(7))]);
        assert_eq!(bot_top_position(&[1, 2], &roles), 7);
        assert_eq!(bot_top_position(&[999], &roles), 0, "unknown roles ignored");
        assert_eq!(bot_top_position(&[], &roles), 0);
    }

    #[test]
    fn filter_skips_roles_at_or_above_bot_managed_and_deleted() {
        let roles = HashMap::from([
            (1, meta(2)),                                  // below bot: ok
            (2, meta(5)),                                  // equal: skipped
            (3, meta(9)),                                  // above: skipped
            (4, RoleMeta { position: 1, managed: true }), // managed: skipped
        ]);
        let (ok, skipped) = filter_assignable(&[1, 2, 3, 4, 5], &roles, 5);
        assert_eq!(ok, vec![1]);
        assert_eq!(skipped, vec![2, 3, 4, 5], "5 was deleted from the guild");
    }

    // --- target_for_user (DB half) ---

    async fn insert_reward(db: &DatabaseConnection, guild_id: i64, role_id: i64, requirement: i32) {
        role_rewards::ActiveModel {
            id: Set(Uuid::now_v7()),
            guild_id: Set(guild_id),
            role_id: Set(role_id),
            requirement: Set(requirement),
            created_at: Set(now()),
            updated_at: Set(now()),
        }
        .insert(db)
        .await
        .expect("insert role reward");
    }

    async fn insert_trophy(db: &DatabaseConnection, guild_id: i64, value: i32) -> Uuid {
        let id = Uuid::now_v7();
        trophies::ActiveModel {
            id: Set(id),
            guild_id: Set(guild_id),
            legacy_id: Set(None),
            creator_user_id: Set(None),
            name: Set(format!("Trophy {id}")),
            normalized_name: Set(normalize_name(&format!("Trophy {id}"))),
            description: Set("d".into()),
            emoji: Set("🏆".into()),
            value: Set(value),
            image: Set(None),
            dedication_user_id: Set(None),
            dedication_text: Set(None),
            details: Set("d".into()),
            signed: Set(false),
            created_at: Set(now()),
            updated_at: Set(now()),
        }
        .insert(db)
        .await
        .expect("insert trophy");
        id
    }

    async fn insert_award(db: &DatabaseConnection, guild_id: i64, user_id: i64, trophy_id: Uuid) {
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
    async fn no_configured_rewards_yields_none() {
        let db = fresh_db().await;
        insert_guild(&db, 1).await;
        assert_eq!(target_for_user(&db, 1, 7).await.unwrap(), None);
    }

    #[tokio::test]
    async fn default_stack_mode_targets_only_the_highest_met_reward() {
        let db = fresh_db().await;
        insert_guild(&db, 1).await;
        insert_reward(&db, 1, 100, 10).await;
        insert_reward(&db, 1, 200, 50).await;
        insert_reward(&db, 1, 300, 1000).await;
        let trophy = insert_trophy(&db, 1, 30).await;
        insert_award(&db, 1, 7, trophy).await;
        insert_award(&db, 1, 7, trophy).await; // score 60

        let (target, configured) = target_for_user(&db, 1, 7).await.unwrap().unwrap();
        assert_eq!(target, vec![200], "stack_roles default 1 = highest only");
        assert_eq!(configured, vec![100, 200, 300]);
    }

    #[tokio::test]
    async fn stack_all_targets_every_met_reward() {
        let db = fresh_db().await;
        insert_guild(&db, 1).await;
        crate::entities::guild_settings::ActiveModel {
            guild_id: Set(1),
            dedication_display: Set(None),
            stack_roles: Set(Some(0)),
            hide_unused_trophies: Set(None),
            hide_quit_users: Set(None),
            leaderboard_format: Set(None),
            created_at: Set(now()),
            updated_at: Set(now()),
        }
        .insert(&db)
        .await
        .expect("insert settings");
        insert_reward(&db, 1, 100, 10).await;
        insert_reward(&db, 1, 200, 50).await;
        let trophy = insert_trophy(&db, 1, 60).await;
        insert_award(&db, 1, 7, trophy).await;

        let (target, _) = target_for_user(&db, 1, 7).await.unwrap().unwrap();
        assert_eq!(target, vec![100, 200]);
    }

    #[tokio::test]
    async fn zero_score_user_targets_no_roles_but_keeps_configured_list() {
        let db = fresh_db().await;
        insert_guild(&db, 1).await;
        insert_reward(&db, 1, 100, 10).await;

        // User with no awards at all: score 0, nothing met — the configured
        // list still comes back so held stale roles get removed.
        let (target, configured) = target_for_user(&db, 1, 7).await.unwrap().unwrap();
        assert!(target.is_empty());
        assert_eq!(configured, vec![100]);
    }
}
