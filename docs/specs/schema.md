# Spec: Database schema (definitive)

Column-level schema for the normalized database. SeaORM migrations in `src/migrations/` implement exactly this; any change goes through this document first. Backed by ADR 0002 (normalized 7-table design, hard deletes with FK cascade), ADR 0003 (portable across SQLite/PostgreSQL — no engine-specific SQL), ADR 0004 (UUIDv7 PKs, Discord snowflakes as `i64`), ADR 0005 (normalized-name uniqueness), ADR 0006 (no stored score).

Conventions: all tables carry `created_at` / `updated_at` timestamps maintained by the application (no DB triggers — not portable). **No soft deletes** (`deleted_at` columns from archived drafts are explicitly rejected; deletion is hard + cascade). UUIDs are generated app-side (`uuid` crate, v7).

## guilds

| Column | Type | Constraints | Notes |
|---|---|---|---|
| id | i64 | PK | Discord snowflake |
| is_safe | bool | NOT NULL default false | legacy `imsafe`; absent in legacy → false |
| created_at / updated_at | timestamp | NOT NULL | |

## trophies

| Column | Type | Constraints | Notes |
|---|---|---|---|
| id | UUID | PK | UUIDv7, app-generated |
| guild_id | i64 | NOT NULL, FK guilds ON DELETE CASCADE | |
| legacy_id | text | NULL | old per-guild string ID ("1".."212"); NULL for post-cutover trophies |
| creator_user_id | i64 | NULL | NULL for the 43 legacy trophies without creator |
| name | varchar(32) | NOT NULL | |
| normalized_name | varchar(64) | NOT NULL | app-maintained: Unicode lowercase, keep alphanumeric of any script; if empty → lowercased raw name (ADR 0005) |
| description | varchar(128) | NOT NULL default text | |
| emoji | varchar(64) | NOT NULL default "🏆" | |
| value | integer | NOT NULL, CHECK −999999..999999 | importer rounds the 44 legacy floats (half-away-from-zero, reported) |
| image | varchar(255) | NULL | opaque filename in `images/` (legacy names kept); NULL = no image |
| dedication_user_id | i64 | NULL | |
| dedication_text | varchar(32) | NULL | set for text-only dedications AND alongside user dedications (stored name) |
| details | varchar(300) | NOT NULL default text | |
| signed | bool | NOT NULL default false | |
| category | varchar(64) | NULL | free-text grouping label; NULL = uncategorized (does not appear on any single-category `/panel medals` panel, but IS shown in its own "Uncategorized" bucket on the `/panel overview` panel) |
| active | bool | NOT NULL default true | inactive medals are excluded from `/award` (autocomplete + direct resolve) but stay visible everywhere else — `/show`, `/trophies`, existing holders |
| created_at / updated_at | timestamp | NOT NULL | created_at = legacy `created` (ms) or synthetic |

Indexes: `UNIQUE(guild_id, normalized_name)` (the ADR 0005 constraint), `(guild_id)`.

## user_trophies — one row per individual award

| Column | Type | Constraints | Notes |
|---|---|---|---|
| id | UUID | PK | UUIDv7 (time-ordered — gives F1 "most recent first" ordering) |
| guild_id | i64 | NOT NULL, FK guilds ON DELETE CASCADE | denormalized on purpose for direct leaderboard queries |
| user_id | i64 | NOT NULL | |
| trophy_id | UUID | NOT NULL, FK trophies ON DELETE CASCADE | |
| awarded_by | i64 | **NULL** | NULL for all imported legacy rows (never tracked); set for new awards (F9) |
| awarded_at | timestamp | NOT NULL | synthetic for imports |
| created_at / updated_at | timestamp | NOT NULL | |

**NO** unique constraint on `(user_id, trophy_id)` — duplicates are required functionality (ADR 0002).
Indexes: `(guild_id, user_id)` (user collections, scores), `(trophy_id)` (cascade, unused-trophy checks), `(guild_id, user_id, trophy_id, awarded_at)` (F1 revoke path).

Score is always `SELECT COALESCE(SUM(t.value),0) FROM user_trophies ut JOIN trophies t ON t.id = ut.trophy_id WHERE ut.guild_id = ? AND ut.user_id = ?`; leaderboard is the same grouped by `user_id` (ADR 0006).

## guild_settings — one row per guild, typed nullable columns

| Column | Type | Constraints | Notes |
|---|---|---|---|
| guild_id | i64 | PK, FK guilds ON DELETE CASCADE | |
| dedication_display | smallint | NULL, CHECK 0..2 | NULL = default 2 |
| stack_roles | smallint | NULL, CHECK 0..1 | NULL = default 1 |
| hide_unused_trophies | smallint | NULL, CHECK 0..1 | NULL = default 1 |
| hide_quit_users | smallint | NULL, CHECK 0..1 | NULL = default 0 |
| leaderboard_format | smallint | NULL, CHECK 0..3 | NULL = default 0 |
| created_at / updated_at | timestamp | NOT NULL | |

NULL = "not explicitly set" → code-side default, mirroring legacy `getSetting` semantics (importer stores only keys present in legacy). Typed columns chosen over key/value rows: 5 fixed settings, compile-time checked.

## role_rewards

| Column | Type | Constraints | Notes |
|---|---|---|---|
| id | UUID | PK | UUIDv7 |
| guild_id | i64 | NOT NULL, FK guilds ON DELETE CASCADE | |
| role_id | i64 | NOT NULL | kept even if the Discord role is deleted (F24: removable without cache) |
| requirement | integer | NOT NULL, CHECK ≥ 1 | |
| created_at / updated_at | timestamp | NOT NULL | |

Indexes: `UNIQUE(guild_id, role_id)` (F22; importer dedupes the 7 legacy duplicate-role guilds keeping the lowest requirement). App-level rules (not DB constraints): max 20 per guild (F23), duplicate `requirement` rejected on add (legacy parity).

## leaderboard_panels

| Column | Type | Constraints | Notes |
|---|---|---|---|
| guild_id | i64 | PK, FK guilds ON DELETE CASCADE | one panel per guild, enforced by PK (F30) |
| channel_id | i64 | NOT NULL | |
| message_id | i64 | NOT NULL | |
| source_guild_id | i64 | NULL | cross-guild link (guild_links): NULL = render `guild_id`'s own leaderboard (default); set = render this OTHER guild's leaderboard instead, while the message still physically lives in `guild_id` |
| created_at / updated_at | timestamp | NOT NULL | updated_at doubles as "last successful render" for the F32 reconciliation sweep |

461 rows at import; many stale (validated at first sweep, not at import).

## active_medals_panels

| Column | Type | Constraints | Notes |
|---|---|---|---|
| id | UUID | PK | UUIDv7, app-generated |
| guild_id | i64 | NOT NULL, FK guilds ON DELETE CASCADE | |
| category | varchar(64) | NOT NULL | the `trophies.category` this panel is scoped to |
| channel_id | i64 | NOT NULL | |
| message_id | i64 | NOT NULL | |
| source_guild_id | i64 | NULL | cross-guild link (guild_links): NULL = render `guild_id`'s own category catalog (default); set = render this OTHER guild's category instead |
| created_at / updated_at | timestamp | NOT NULL | updated_at doubles as "last successful render" (same convention as leaderboard_panels) |

Indexes: `UNIQUE(guild_id, category)` — one panel per category per guild (the multi-row analogue of `leaderboard_panels`' single-row-per-guild PK). Rendered content: every `trophies` row in the EFFECTIVE guild (`source_guild_id` if set, else `guild_id`) + category with `active = true`, name + description, no score data — a live catalog, not a leaderboard. Refreshed on trophy create/edit/delete (category, active, name, emoji or description changed) in the effective guild, never on award/revoke/clear.

## medals_overview_panels

| Column | Type | Constraints | Notes |
|---|---|---|---|
| guild_id | i64 | PK, FK guilds ON DELETE CASCADE | one overview panel per guild, enforced by PK (same convention as `leaderboard_panels`) |
| channel_id | i64 | NOT NULL | |
| message_id | i64 | NOT NULL | |
| source_guild_id | i64 | NULL | cross-guild link (guild_links): NULL = render `guild_id`'s own catalog (default); set = render this OTHER guild's instead |
| created_at / updated_at | timestamp | NOT NULL | updated_at doubles as "last successful render" |

Rendered content: every ACTIVE trophy in the effective guild, one embed field per category (alphabetical, capped at Discord's 25-field limit) plus a trailing "Uncategorized" field for active trophies with no category — each field lists its medals the same way `active_medals_panels` does. Refreshed by the exact same trigger as `active_medals_panels` (any category/active/name/emoji/description change) and swept the same way (F32-style) — piggybacks on `medals_panel.rs`'s existing debounce/sweep rather than a separate signal channel.

## guild_links

| Column | Type | Constraints | Notes |
|---|---|---|---|
| id | UUID | PK | UUIDv7, app-generated |
| source_guild_id | i64 | NOT NULL, FK guilds ON DELETE CASCADE | the data owner ("guild A") |
| linked_guild_id | i64 | NOT NULL, FK guilds ON DELETE CASCADE | the co-administering guild ("guild B") — once accepted, every trophy-content command run in B (`util::effective_guild_id`) operates on `source_guild_id`'s data instead of its own |
| requested_by | i64 | NOT NULL | user who ran `/link request` |
| accepted_by | i64 | NULL | user who ran `/link accept`; NULL while pending |
| accepted_at | timestamp | NULL | **NULL = pending, set = accepted** — doubles as the status flag, no separate enum column |
| created_at / updated_at | timestamp | NOT NULL | |

Indexes: `UNIQUE(linked_guild_id)` — a guild can be the *linked* side of at most one row (pending or accepted) at a time, so each B mirrors exactly one A; `(source_guild_id)` — non-unique, since one A can be linked into many guilds' panels (one-to-many). `source_guild_id != linked_guild_id` is enforced app-side (cross-column CHECK constraints aren't portable via the SeaORM schema API), the same class of app-level rule as `role_rewards`' 20-per-guild cap.

Every panel render that has a `source_guild_id` set re-validates against this table (an `accepted_at IS NOT NULL` row must exist for that exact `(source_guild_id, linked_guild_id)` pair) BEFORE rendering — not just trusting the panel's stored column — so a revoked link stops leaking data on the very next refresh even if cleanup at revoke time were ever incomplete.

## bot_stats — key/value counters

| Column | Type | Constraints | Notes |
|---|---|---|---|
| id | integer | PK auto-increment | (existing table shape) |
| name | varchar | NOT NULL UNIQUE | e.g. `award`, `total`, `trophiesAwarded` (historical), `rootTrophies` (historical) |
| total | i64 | NOT NULL default 0 | new counters count successful executions only (§2 parity) |
| created_at / updated_at | timestamp | NOT NULL | |

Legacy counters are imported once as historical record and never used for validation.

## Portability notes (ADR 0003)

- UUID: native `uuid` in PostgreSQL; BLOB/TEXT in SQLite — SeaORM abstracts; tests must not compare raw storage.
- Timestamps app-generated (`updated_at` set by application code, not triggers).
- `CHECK` constraints expressed via SeaORM schema API; where unsupported, enforced app-side and documented here.
- No `SERIAL`, no `plpgsql`, no partial indexes.

## Rejected ideas from archived drafts (do not resurrect)

- `SERIAL` integer PKs and integer `trophy_id` FKs → replaced by UUIDv7 (ADR 0004).
- `awarded_by BIGINT NOT NULL` → must be NULL-able (imports).
- `deleted_at` soft-delete columns + partial indexes → hard delete + cascade (ADR 0002).
- Exact-match `UNIQUE(guild_id, name)` → normalized uniqueness (ADR 0005).
- `guilds.name` column → not needed by any command.
- 8th table `command_logs` + triggers → out of scope; would need an ADR.
