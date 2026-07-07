# Spec: Legacy data model (quick.db)

Validated against the production dumps `bot_db.json` / `guilds_db.json` (2025-10-27 snapshot), the files on disk in `./images/`, and the Node.js source. Measurements re-verified by an independent data audit on 2026-07-08. This is the authoritative description of what the importer will receive.

## Storage layout

quick.db uses one SQLite file (`json.sqlite`) with two tables, `bot` and `guilds`. Each table has a single row whose `json` column holds one giant JSON document. All access in the legacy bot is by path notation (`data.${guild}.trophies.${id}`). The Rust loader is `src/legacy/mod.rs`.

## Verified production statistics

| Metric | Value |
|---|---|
| Root guild keys | 2,493 |
| `/forgetme` tombstones (guild value is the literal integer `-1`) | 5 |
| Valid guild objects | 2,488 |
| Trophies created (real count) | 10,853 |
| Current awards (sum of user array lengths; all elements are strings) | 60,554 |
| Orphaned awards (referencing nonexistent trophy IDs) | **0** |
| Users with a `trophies` array | 9,583 (8,299 non-empty, 1,284 empty — empty records also come from no-op revokes, which wrote `{trophies: [], trophyValue: 0}` for unknown users) |
| Users whose stored `trophyValue` ≠ recalculated raw sum (float-tolerant) | 51 (drift −990..+3,500); vs the importer's ROUNDED values the total is 133 — see migration-import.md |
| Guilds with zero trophies | 393 |
| Duplicate-name groups (Unicode-aware normalization + emoji fallback, ADR 0005) | 287 groups / 643 trophies / 209 guilds |
| Names >32 chars as stored | 0 (22 would exceed 32 with the dedupe suffix; 21 under the final rule) |
| Names empty after normalization (emoji/symbol-only) | 17 (2 exact raw duplicates among them) |
| Trophies with **non-integer float** `value` (e.g. 8.5, 0.42) | 44, across 19 guilds |
| Users with float `trophyValue` (accumulation artifacts) | 60 |
| Guilds with a leaderboard `panel` | **461** |
| Guilds with non-empty `rewards` | 135 (287 entries; 7 guilds contain the SAME role 2–3 times) |
| Trophy images: `null` / local filename / CDN URL | 7,965 / 2,693 / 195 (all URLs on cdn.discordapp.com) |
| Referenced local image files **missing from disk** | 200 (across 130 guilds) |
| Image files on disk | 2,771 (278 orphans; 124 of those have a signed-URL query string baked into the filename) |
| Local images with extensions outside PNG/JPG/JPEG/GIF | ~60 (webp 41, jfif 6, avif 3, mov 3, mp4 3, svg 1, xcf 1, exe 1, other 1) |
| Max awards for a single user | 2,009 (3 distinct trophies — legitimate) |

Global counters in `bot` data are unreliable (cumulative-only bugs): it reports 10,571 trophies (real 10,853) and 120,411 awards (real 60,554). Never validate against them.

## `bot` document

```json
{
  "version": 0,
  "defaultLanguage": "en",
  "bannedUsers": [],
  "commands": { "total": 104913, "award": 41240, "...": 0 },
  "trophiesAwarded": 120411,
  "trophies": 10571,
  "lastDay": 18,
  "milestone": true
}
```

Import decisions, field by field: `commands.*`, `trophies`, `trophiesAwarded` → `bot_stats` as historical record (ADR 0006). `version`, `defaultLanguage`, `bannedUsers` (empty in production), `lastDay`, `milestone` → **not migrated** (vestigial). `commands.total` exactly equals the sum of the per-command counters.

## `guilds` document

Root object: keys are guild snowflake strings (all numeric). Five entries hold the literal integer **`-1`** instead of an object — these are `/forgetme` **tombstones** (globals.js:380 writes `-1` instead of deleting; the legacy bot even "resurrects" them with a fresh record on the next interaction). The importer skips them as deliberately deleted data → report `tombstoned_guilds`.

### Guild object

```json
{
  "id": "<same as key>",
  "imsafe": true,
  "language": "en",
  "restapi": { "token": "", "enabled": false },
  "settings": { "dedication_display": 2 },
  "trophies": { "current": 12, "1": { ... }, "7": { ... } },
  "users": { "<user_id>": { "trophies": ["1", "1", "7"], "trophyValue": 35 } },
  "rewards": [ { "role": "<role_id>", "requirement": 100 } ],
  "permissions": { "manage_trophies": ["<role_id>"] },
  "panel": { "message": "<message_id>", "channel": "<channel_id>" }
}
```

Real presence (out of 2,488 valid guilds): `id`, `language` (always `"en"`), `settings`, `trophies`, `users`, `rewards`, `permissions` are present in ALL of them; `restapi` in 2,387 (always the inert `{"token": "", "enabled": false}`); `imsafe` in 2,407 (always `true`, never `false`; treat absence as `false`); `panel` in 461. One guild (`1316734441187577966`) carries a typo key `tropies` (`{"current": 1}`) alongside its intact real `trophies` map.

Not migrated: `id` (redundant), `language` (always "en", system never worked), `restapi` (inert vestige), `permissions` (deprecated system; non-empty in only 17 guilds). Typed serde structs must **tolerate unknown fields** (no `deny_unknown_fields`) and should not log the four ubiquitous keys above as noise.

### Trophies map

- Keys are stringified per-guild counters (`"1"`..`"212"`) **plus the special key `"current"`** holding the next-ID counter — always skip it. `current` is never behind the max ID in production.
- Keys are unique per guild by JSON construction: same-ID duplicates cannot exist. Name duplicates can and do (see statistics; handling in ADR 0005).

Trophy object and field presence (out of 10,853):

| Field | Notes | Presence |
|---|---|---|
| `creator` | user snowflake string, always numeric when present | 99.6% (43 missing) |
| `created` | Unix ms timestamp; all present values plausible (2015+..now) | 99.6% (43 missing) |
| `name` | ≤32 chars (0 violations) | 100% |
| `description` | ≤128 chars (default text common) | ~100% |
| `emoji` | ≤64 chars, default `:trophy:` | ~100% |
| `value` | **number** −999,999..999,999 (both extremes exist); **44 are non-integer floats** | 100% |
| `image` | see shapes below | 26.6% non-null |
| `dedication` | 10,114 null/empty, 496 text-only, 243 user (always with name) | 6.8% non-empty |
| `details` | ≤300 chars | 96.7% (360 missing; 9,012 default text) |
| `signed` | bool | 99.6% (43 missing) |

The 43 trophies missing `creator`/`created`/`signed`/`details` are pre-rewrite legacy; the importer applies defaults (creator → NULL, synthetic timestamp, signed=false, default details).

**`dedication` shapes (4):** absent or `{}`; `{"user": null, "name": null}`; `{"user": null, "name": "free text"}`; `{"user": "<id>", "name": "<username>"}`. All `user` values parse as u64.

**`image` shapes (3):** `null` (7,965); local filename `"{guild_id}_{legacy_trophy_id}.{ext}"` (2,693 — but 200 referenced files are missing from disk); full `https://cdn.discordapp.com/...` URL (195 — signed and expiring). Historical extension validation was NOT enforced: ~60 local references are webp/jfif/avif/mov/mp4/svg/xcf/exe — the new bot must serve (or gracefully skip) them without choking.

### Users map

All user keys are numeric snowflakes; `trophies` array elements are all strings; duplicates are awards (one element = one award). **Production currently has zero orphaned award references** — the importer still keeps the orphan-handling path (defense) but validation expects `orphaned == 0`. `trophyValue` is denormalized: 54 users drift from the recalculated sum (range −990..+3,500) and 60 users carry float values (e.g. `8.200000000000003`); it is used only for the validation report (ADR 0006), which must compare float-tolerantly.

### Rewards, panel, settings

- `rewards`: 287 entries in 135 guilds. All role IDs numeric, all requirements integers ≥1, no guild over 20 entries. **7 guilds repeat the same role ID 2–3 times** (legacy duplicate bug) — importer dedupes (see migration-import.md).
- `panel`: 461 guilds, all exactly `{"message": "<digits>", "channel": "<digits>"}`. Many targets will be stale (deleted channels/messages) — the reconciliation sweep must expect that on day one.
- `settings`: present in all guilds, non-empty in 162. Zero out-of-range values, zero unexpected keys. Defaults when keys are missing: `dedication_display: 2`, `stack_roles: 1`, `hide_unused_trophies: 1`, `hide_quit_users: 0`, `leaderboard_format: 0` (validated in commands-admin.md against globals.js).

## Known legacy bugs affecting the data

- `revoke.js` calls `Array.pop(id)` — the argument is ignored, so it always removed the LAST award, not the requested trophy. Existing arrays already contain the results of wrong revocations; they are migrated as-is (they are the real state). The Rust bot fixes the behavior.
- Counters (`trophies`, `trophiesAwarded`) never decremented on revoke/clear → inflated (see statistics).
- No name-uniqueness validation on `/create` → duplicate names (ADR 0005).
- `/create` accepted non-integer values → 44 float-valued trophies; `/edit` stored expiring CDN URLs instead of downloading → the 195 URL images; failed downloads/deletes left the 200 missing files and 278 orphan files (124 with query-string filenames).
- Role rewards: dead under discord.js v14 **except** in guilds where the bot has Administrator (the v14 permission check short-circuits) — so real, live role-reward behavior exists in part of production. See core-behaviors.md.
