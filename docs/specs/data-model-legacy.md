# Spec: Legacy data model (quick.db)

Validated against the production dumps `bot_db.json` / `guilds_db.json` (2025-10-27 snapshot) and the Node.js source. Analysis date: 2026-07-07. This is the authoritative description of what the importer will receive.

## Storage layout

quick.db uses one SQLite file (`json.sqlite`) with two tables, `bot` and `guilds`. Each table has a single row whose `json` column holds one giant JSON document. All access in the legacy bot is by path notation (`data.${guild}.trophies.${id}`). The Rust loader is `src/legacy/mod.rs`.

## Verified production statistics

| Metric | Value |
|---|---|
| Guilds in the blob | 2,493 |
| Guild entries that are NOT objects (corrupt, value is an int) | 5 |
| Trophies created (real count) | 10,853 |
| Current awards (sum of user array lengths) | 60,554 |
| Users with trophies | 8,299 |
| Users with empty `trophies: []` | 1,284 |
| Guilds with zero trophies | 393 |
| Guilds with exact duplicate trophy names | 176 (7%) |
| Guilds with case/whitespace-insensitive duplicate names | 198 |
| Trophy image files on disk (`./images/`) | ~2,771 |
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

Imported into `bot_stats` as historical record only (ADR 0006).

## `guilds` document

Root object: keys are guild snowflake strings, values are guild objects — **except 5 entries whose value is a plain integer** (quick.db corruption). The importer must skip and report these.

### Guild object

```json
{
  "imsafe": true,
  "language": "en",
  "settings": { "dedication_display": 2 },
  "trophies": { "current": 12, "1": { ... }, "7": { ... } },
  "users": { "<user_id>": { "trophies": ["1", "1", "7"], "trophyValue": 35 } },
  "rewards": [ { "role": "<role_id>", "requirement": 100 } ],
  "permissions": { "manage_trophies": ["<role_id>"] },
  "panel": { "message": "<message_id>", "channel": "<channel_id>" }
}
```

Every field is optional in practice. `settings` may be `{}` or partial (missing keys fall back to defaults in `globals.js`). `permissions` belongs to the deprecated system and is not migrated. `language` is not migrated.

### Trophies map

- Keys are stringified per-guild counters (`"1"`..`"212"`) **plus the special key `"current"`** holding the next-ID counter — always skip it.
- Keys are unique per guild by JSON construction: same-ID duplicates cannot exist. Name duplicates can and do (see stats above; handling in ADR 0005).

Trophy object and field presence (out of 10,853):

| Field | Notes | Presence |
|---|---|---|
| `creator` | user snowflake string | 99.6% (43 missing) |
| `created` | Unix ms timestamp | 99.6% (43 missing) |
| `name` | ≤32 chars | 100% |
| `description` | ≤128 chars | ~100% (default text common) |
| `emoji` | ≤64 chars, default `:trophy:` | ~100% |
| `value` | int, −999,999..999,999 (extremes exist in production) | 100% |
| `image` | see shapes below | 26.6% non-null |
| `dedication` | see shapes below | 6.8% non-empty |
| `details` | ≤300 chars | 96.7% (360 missing; 9,012 are the default text) |
| `signed` | bool | 99.6% (43 missing) |

The 43 trophies missing `creator`/`created`/`signed`/`details` are pre-rewrite legacy; the importer applies defaults (creator 0/unknown, synthetic timestamp, signed=false, default details).

**`dedication` shapes (4):** absent or `{}`; `{"user": null, "name": null}`; `{"user": null, "name": "free text"}`; `{"user": "<id>", "name": "<username>"}`.

**`image` shapes (3):** `null`; local filename `"{guild_id}_{legacy_trophy_id}.{ext}"` under `./images/`; full external URL (`https://cdn.discordapp.com/...`). Stored as an opaque string; files are NOT renamed at migration (filenames keep legacy trophy IDs).

### Users map

`trophies` is an array of trophy-ID **strings**; duplicates are awards (one array element = one award). Entries may reference trophy IDs that no longer exist in the guild's trophies map (orphaned awards — the legacy `cleanseTrophies` did not always run); the importer counts and reports them, and they are dropped (no FK target). `trophyValue` is denormalized and possibly wrong; used only for the validation report (ADR 0006).

### Settings defaults

When keys are missing: `dedication_display: 2`, `stack_roles: 1`, `hide_unused_trophies: 1`, `hide_quit_users: 0`, `leaderboard_format: 0`. Authoritative list with option meanings: `docs/specs/commands-admin.md` (validated against `globals.js`).

## Known legacy bugs affecting the data

- `revoke.js` calls `Array.pop(id)` — the argument is ignored, so it always removed the LAST award, not the requested trophy. Existing arrays already contain the results of wrong revocations; they are migrated as-is (they are the real state). The Rust bot fixes the behavior.
- Counters (`trophies`, `trophiesAwarded`) never decremented on revoke/clear → inflated (see statistics).
- No name-uniqueness validation on `/create` → duplicate names (ADR 0005).
