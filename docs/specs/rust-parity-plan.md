# Spec: Rust parity & remediation plan

The contract for the rewrite: the Rust bot ships with **exactly the same commands and features** as the production Node.js bot — but working correctly, scalable and maintainable. This document consolidates every validated defect into its fix, lists the few intentional behavior deltas (all ADR-backed), and defines acceptance criteria per command. Detailed per-command behavior lives in the other specs; this is the master checklist.

## Principles

1. **Feature parity, not code parity.** Same commands, same parameters, same user-visible features. Internal design follows the ADRs, never the legacy implementation.
2. **BUGs are fixed, never reproduced.** A BUG is behavior that contradicts the command's own intent (wrong trophy revoked, crash, dead code that was supposed to run).
3. **Intentional deltas are explicit.** Anything that changes user-visible behavior on purpose is listed in §4 and backed by an ADR. If it's not there, legacy behavior wins.
4. **Every fix is testable.** Acceptance criteria in §6; `cargo test` must cover the fix catalog.

## 1. Command parity table

| Legacy command | In Rust | Notes |
|---|---|---|
| `/about` `/help` `/invite` `/ping` `/stats` `/suggest` `/support` | ✅ same | `/help` content rewritten (drops deprecated permissions lesson) |
| `/imsafe` | ✅ kept as no-op confirmation | Gate itself retired (Discord native permissions); command replies "already safe" for continuity |
| `/forgetme` | ✅ same UX (owner + confirm button) | Now actually deletes (cascade) instead of tombstone |
| `/create` `/edit` `/delete` `/award` `/revoke` `/clear` `/details` | ✅ same parameters | Fix catalog §3 applies |
| `/export` | ✅ same | Exports normalized data as JSON |
| `/panel create/delete` | ✅ same | Better refresh model (§3.4) |
| `/permissions add/list/remove` | ✅ kept at cutover | Same static deprecation notice; candidate for removal in a later release |
| `/rewards add/remove/clear/list` | ✅ same | Role application finally works (§3.1) |
| `/settings set/list` | ✅ same 5 settings, same defaults | Value input via proper Discord choices |
| `/leaderboard` `/show` `/trophies user/guild` | ✅ same | Fix catalog §3 applies |
| `/language`, `/trophystats` | ❌ not reimplemented | Dead files, never functional |

Trophy selection everywhere: by **name** (unique per guild, ADR 0005) with slash-command autocomplete, replacing the legacy numeric-ID + substring matching.

## 2. Cross-cutting fixes (affect every command)

| Legacy defect | Rust fix |
|---|---|
| No reply is ever ephemeral (dispatcher always defers publicly) | Per-command ephemeral where intended: `/invite`, `/support`, `/details`, `/export`, and all error replies |
| Cooldowns declared but never enforced | Poise built-in cooldowns (`/stats`, `/suggest`: 10s) |
| Admin/permission checks use v13 flag strings → always false or throwing | Serenity typed `Permissions`; checked at registration (`default_member_permissions`) and defensively at runtime |
| Role rewards dead under d.js v14 **except in guilds where the bot has Administrator** (v14 `has()` short-circuits before the invalid v13 flag resolves) — so live reward behavior exists in part of production; the unawaited async call is also a process-crash vector on hierarchy failures | Working reward engine everywhere with just ManageRoles: on award/revoke/clear, recompute the user's target role set from score + `stack_roles` setting, diff against current roles, apply respecting hierarchy — awaited, idempotent, errors logged, never swallowed |
| Command counters increment even on error | Count successful executions; errors logged separately with context |
| Errors silently swallowed (`catch {}` everywhere), while UNCAUGHT async rejections (bare event handlers, un-awaited calls) crash the whole process under Node 18 | Every event/dispatch path is wrapped: errors log via `log` with guild/user/command context and answer the user with a friendly error embed; no code path can take the process down |
| Stored `trophyValue` desyncs | No stored score — computed `SUM` per ADR 0006 |
| Whole-blob rewrites for any change | Row-level SQL in transactions |

## 3. Per-command fix catalog

### 3.1 Trophy management

| # | Defect (validated) | Fix |
|---|---|---|
| F1 | `/revoke` removes the LAST award regardless of trophy (`pop(id)`), desyncing array vs score | Delete N `user_trophies` rows of the REQUESTED trophy (most recent first); score is computed so it can never desync |
| F2 | `/revoke` reports "removed all" even when the user had 0 copies | Correct feedback: revoked count, or explicit "user does not have this trophy" |
| F3 | `/create` persists the trophy BEFORE validating the image → half-created trophies, consumed IDs | Validate everything first (name, limits, image content-type and ≤1MB), download the image, then insert in a single transaction |
| F4 | `/create` accepts float values | Integer option, range −999,999..999,999 enforced by Discord + server-side |
| F5 | `/create` allows duplicate names | Uniqueness check (normalized, ADR 0005) with a clear error |
| F6 | `/edit` stores the expiring Discord CDN URL instead of the file; 1MB check dead | Same image pipeline as `/create`: download, validate, store filename |
| F7 | `/edit` cannot change `value`/`signed` (undocumented) | Kept immutable at cutover (parity); documented; candidate for a later release |
| F8 | `/award` coerces count <1 to 1 with a misleading message | Reject out-of-range count (1–50) with a clear error |
| F9 | `/award` never records who awarded | `user_trophies.awarded_by` recorded (NULL only for imported legacy rows) |
| F10 | `/delete` never recomputes role rewards and unlinks `./images/null` when imageless | FK cascade removes awards; reward recompute triggered for affected users; image unlink only when an image exists, errors logged |
| F11 | `/details` public + bypasses permission gate | Ephemeral reply; Manage Guild permission enforced like its siblings |
| F12 | `getTrophy` path traversal (`1.name` "resolves"; `/award` then persists the bogus ID into the user's array before quick.db's `add` throws on NaN and aborts the command), substring matching, emoji-only input matches everything | Exact normalized-name resolution + autocomplete; parameterized queries make traversal impossible |
| F37 | `/edit` change report cosmetics: editing a dedication to the same value counts as a change; removing with `-` prints "> null" | Accurate change report (cosmetic; fixed as part of the `/edit` response formatting) |

### 3.2 User-facing

| # | Defect | Fix |
|---|---|---|
| F13 | `/leaderboard` AND the panel renderer crash on departed users with formats 1–3 (uncaught fetch; panels silently stop updating) | Shared rendering path with fallback to stored/"Unknown user" display; never crash |
| F14 | Rank numbers/medals computed from the raw unclamped `page` option | Clamp page first; ranks always consistent |
| F15 | Zero-score users excluded by truthiness; total = visible rows only | Users with awards appear even at score 0; total = real server total |
| F16 | Quit-user detection depends on gateway cache heuristics | Explicit membership check with documented fallback; `hide_quit_users` setting honored deterministically |
| F17 | `/show` throws if the local image file is missing | Graceful fallback to the default trophy image; warning logged |
| F18 | `/trophies user` shows "undefined's Trophies" on failed fetch | Proper fallback name |
| F19 | `/trophies guild` manager/admin exemptions are dead code | Implement the documented INTENT: users with Manage Guild see unused trophies regardless of the setting |
| F20 | Orphaned award IDs silently skipped in displays but counted in stored score | Impossible by FK; importer drops+reports them (migration-import.md; production currently has 0) |
| F36 | Dedication display mode 1 ("Always Name") never shows the live username (`GuildMember.username` undefined in v14) — always falls back to the stored creation-time name | Implement mode 1 correctly: resolve the live display name, fall back to the stored `dedication_text` |

### 3.3 Administration

| # | Defect | Fix |
|---|---|---|
| F21 | `/rewards add` hierarchy check always false (operator precedence) | Correct check: non-owners cannot add roles ≥ their highest role |
| F22 | Duplicate reward roles allowed (string vs object comparison); a duplicated role was then effectively ALWAYS stripped, because `doRewardRoles` applies additions before removals and the role landed in both lists | UNIQUE(guild_id, role_id); clear error on duplicates; reward engine computes one final target set per user (no add-then-remove ordering hazard) |
| F23 | Reward limit is 21 (off-by-one), docs said 20 | Exactly 20 enforced on new adds; the importer applies no cap (production max is below 20, but legacy guilds could legitimately hold 21) |
| F24 | Reward for a deleted role cannot be removed (requires cache hit) | Remove by stored role ID; no cache requirement; `/rewards list` marks deleted roles |
| F25 | `/rewards` command/subcommand description strings are wrong in source | Correct descriptions |
| F26 | `/settings set` substring matching ("2abc" parses, "mention" first-match) | Discord native choices per setting; no free-text parsing |
| F27 | Unknown setting id would crash (`&&`/`||` guard bug) | Impossible via typed enum; defensive error anyway |
| F28 | `/export` public attachment of raw blob, leftover `console.log` | Ephemeral reply; normalized, versioned JSON export |

### 3.4 Panels & background work

| # | Defect | Fix |
|---|---|---|
| F29 | Panel refresh only via loop: 60s + 1s/guild ≈ 42 min full cycle at production scale | Event-driven update (debounced) on award/revoke/clear in that guild, plus a low-frequency reconciliation sweep |
| F30 | Second `/panel create` orphans the previous message; delete leaves the message | On create: delete the old panel message first; on delete: delete the Discord message too (best effort, logged) |
| F31 | Failed panel render leaves a "Creating panel..." stub recorded | Transactional: DB record only after successful send |
| F32 | Stale panels (deleted channel/message) never cleaned | Reconciliation sweep removes records pointing at dead targets |
| F33 | `/forgetme` tombstone `-1` instead of deletion; unlink errors swallowed | True cascade delete of all guild rows + image files; each failure logged; then leave guild |
| F34 | `/stats` reports inflated cumulative counters and cache-size "users" | Live counts computed from the DB (guilds, trophies, awards) + accurate process stats |
| F35 | `/ping` measures interaction→defer time | Real gateway ping + measured round trip |

## 4. Intentional behavior deltas (not bugs — ADR-backed)

Users may notice these; everything else must feel identical.

1. **Trophy identification**: name + autocomplete instead of numeric IDs; footers show the name, not "Trophy ID: N" (ADR 0004/0005).
2. **Renamed legacy duplicates**: 641 colliding trophy names across 209 guilds get their legacy ID suffixed at import (ADR 0005; per-guild report). Normalization is Unicode-aware so non-Latin names are NOT falsely renamed.
3. **Scores are always exact**: computed from awards; the 54 users whose stored `trophyValue` had drifted will see the corrected number, and the 44 float-valued trophies are rounded to integers (ADR 0006; import report).
4. **Role rewards work everywhere** — under d.js v14 they only worked in guilds where the bot had Administrator (dead elsewhere). After cutover the engine works with plain ManageRoles; in previously-dead guilds users will see reward roles appear on their first score change. Existing role state in Administrator guilds is respected: the recompute is idempotent.
5. **The 150-trophies-per-guild limit is kept at cutover** for parity, but is now a config value, not a technical ceiling (ADR 0002).
6. **`/imsafe` gate retired**: management commands rely on Discord permissions only.
7. **Ephemeral where it was always intended** (§2) — some replies that used to be public become private.

## 5. Scalability & maintainability requirements

- **No global state blobs**: every operation touches only its guild's rows, in a transaction. Concurrent guilds never contend.
- **Indexed access paths**: (guild_id) on every table; (guild_id, user_id) on `user_trophies`; normalized-name unique index on `trophies`.
- **Leaderboard = one query** (`SUM ... GROUP BY user ORDER BY total DESC LIMIT/OFFSET`); no full-dataset scans in command paths (legacy `/trophies guild` unused-filter was O(users × trophies)).
- **Modular code** (CLAUDE.md rules): one module per command area, shared logic in dedicated modules, `main` minimal; `log` crate only.
- **Tests are part of parity**: every F-item in §3 gets at least one test; the import gets count/score validation tests against fixture data shaped like the real anomalies (data-model-legacy.md).
- **Graceful shutdown** (ADR 0009) covers background workers (panel updater) too.

## 6. Acceptance criteria

Ordered by current staging: items 2–3 are the near-term goal (code + import proven locally on SQLite); items 1, 4 and 5 gate the eventual cutover, which is deferred until command parity is complete.

1. All commands in §1 registered and responding on a test guild with production-shaped imported data.
2. Import report reviewed: counts match (10,853 trophies, 60,554 awards − orphans), renames and score corrections audited (migration-import.md).
3. Every §3 fix demonstrated by a test; `cargo test` green.
4. Smoke script on the test guild: create → award ×N → leaderboard → revoke (correct trophy!) → role reward applied → clear → panel updates.
5. Legacy Node.js bot remains deployable for 24h rollback.
