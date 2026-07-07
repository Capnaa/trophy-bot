# Command index — complete validated list

Every slash command of the production bot, what it REALLY does (validated against the JS source, not the old docs), its main defects, and where its full spec lives. Cross-cutting defects that affect all commands are listed at the bottom.

**Registered commands: 24** (plus 2 dead files that were never functional).

## Bot utility — [commands-utility.md](commands-utility.md)

| Command | Real behavior | Defects to fix in Rust |
|---|---|---|
| `/about` | Static info embed (GitHub, Ko-fi, support links) | — |
| `/forgetme` | Owner-only + confirm button; deletes trophy image files, overwrites guild data with a `-1` tombstone (not a real delete), leaves the guild | Real deletion; unlink errors silently swallowed |
| `/help` | Static usage guide | Content still teaches the deprecated custom permission system |
| `/imsafe` | One-way flag `imsafe=true`; only gates commands declaring legacy `permissions`; new guilds are already created safe | Gate retired; command kept as a no-op confirmation |
| `/invite` | Static OAuth2 URL | Meant to be ephemeral, actually public |
| `/ping` | WS ping + interaction→defer time (not a real round trip) | Measure real latency |
| `/stats` | Global counters (known inflated) + cache sizes + uptime | Compute from real DB data; 10s cooldown declared but never enforced |
| `/suggest` | Static redirect to support server | Cooldown never enforced |
| `/support` | Static support/GitHub embed | Meant ephemeral, actually public |
| `/language` | **Dead** — fully commented out, never loads | Not reimplemented |

## Trophy management — [commands-trophy-management.md](commands-trophy-management.md)

| Command | Real behavior | Defects to fix in Rust |
|---|---|---|
| `/create` | Creates trophy (name ≤32, desc ≤128, emoji ≤64, value ±999,999, image PNG/JPG/JPEG/GIF ≤1MB, ≤150/guild); auto-increment per-guild string ID | Persists BEFORE image validation (half-created trophies); no duplicate-name check (ADR 0005); floats accepted as value |
| `/edit` | Partial edit merged with `\|\|`; value and signed are immutable | Stores the expiring CDN URL instead of downloading the image; 1MB check is dead code |
| `/delete` | Deletes trophy, removes it from every user (full blob rewrite), unlinks image | No confirmation; never recomputes role rewards; FK cascade replaces the manual sweep |
| `/award` | Pushes trophy ID ×count (1–50; <1 silently coerced to 1) onto user array, adds value to stored score | Awarder never recorded → `awarded_by`; validate count properly |
| `/revoke` | **BUG:** `Array.pop(id)` ignores the argument — always removes the LAST award (possibly another trophy) while subtracting the requested trophy's value → data desync | Fixed: remove N occurrences of the requested trophy |
| `/clear` | Resets user to `trophies=[]`, score 0 | Wrong option description in source |
| `/details` | Shows the trophy's PRIVATE details field | Reply is public, not ephemeral; bypasses the imsafe gate |

**Trophy resolution (`getTrophy`)**: numeric input = raw ID lookup (with a path-traversal bug: `1.name` "resolves"); otherwise case/punctuation-insensitive SUBSTRING match on normalized names, lowest ID wins. Rust replaces this with exact unique names + autocomplete (ADR 0005).

## Server administration — [commands-admin.md](commands-admin.md)

| Command | Real behavior | Defects to fix in Rust |
|---|---|---|
| `/export` | Admin-only; dumps raw guild JSON as a public attachment | Should be ephemeral; export normalized data |
| `/panel create/delete` | Persistent leaderboard message; refreshed only by a background loop (60s + 1s/guild ≈ 42 min full cycle at 2,500 guilds) | Creating a 2nd panel orphans the old message; delete leaves the message; 0-score users hidden |
| `/permissions add/list/remove` | Static deprecation notice only | Not reimplemented |
| `/rewards add/remove/clear/list` | Role rewards by score threshold (min 1), sorted desc, list 5/page | Hierarchy check always false (operator precedence); duplicate roles allowed; limit is 21 not 20; deleted-role rewards unremovable; **and role assignment itself is dead under d.js v14** (see below) |
| `/settings set/list` | 5 settings stored as 0-based index; accepts option number or substring name | Defaults validated: `dedication_display=2, stack_roles=1, hide_unused_trophies=1, hide_quit_users=0, leaderboard_format=0` |

## User-facing — [commands-user.md](commands-user.md)

| Command | Real behavior | Defects to fix in Rust |
|---|---|---|
| `/leaderboard` | Stored `trophyValue` desc, 10/page, medals top-3, quit-user filter (cache-based) | Rank numbers use the raw unclamped page option; CRASHES on departed users with formats 1–3; zero-score users excluded by truthiness; total = visible rows only. Rust: score computed via SUM (ADR 0006) |
| `/show` | Public trophy display; image = local file / URL / default CDN fallback | Crashes if the local image file is missing; 195 production trophies hold expiring CDN URLs |
| `/trophies user` | Collection aggregated as `×N` per trophy, sorted by value | Orphaned award IDs silently skipped (but still counted in stored score) |
| `/trophies guild` | All guild trophies by value desc; unused-trophy hiding | Manager/admin exemption checks are dead code — the setting alone decides |
| `/trophystats` | **Dead** — 0-byte file, never registered | Not reimplemented |

## Cross-cutting defects (affect everything) — [core-behaviors.md](core-behaviors.md)

- **Role rewards are dead under discord.js v14 EXCEPT in guilds where the bot has Administrator** (v14 permission check short-circuits before the invalid v13 flag resolves) — live behavior exists in production and the unawaited call is a process-crash vector; see core-behaviors.md.
- **No reply is ever ephemeral**: the dispatcher always defers publicly.
- **Cooldowns are never enforced** (infrastructure exists, nothing checks it).
- **Admin detection always false** (v13 `'ADMINISTRATOR'` string compared under v14).
- Command counters increment even when the command errors; new guilds seeded with `imsafe: true`; button pagination described in old docs does not exist.

## Fixing it all in Rust

The consolidated remediation plan — command parity table, every defect above mapped to its fix (F1–F37), the intentional ADR-backed behavior deltas, and the cutover acceptance criteria — lives in [rust-parity-plan.md](rust-parity-plan.md). That is the master checklist for implementation. The definitive column-level database schema is in [schema.md](schema.md).

## Data handling

How data is stored, manipulated and migrated:

- Per-command quick.db reads/writes: "Data operations" section inside each spec above.
- Legacy structures, verified production statistics and anomalies: [data-model-legacy.md](data-model-legacy.md).
- Conversion into the normalized schema (UUIDv7 mapping, name dedupe, orphan handling, validation report): [migration-import.md](migration-import.md).
