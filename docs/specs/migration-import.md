# Spec: Legacy data import

How production data moves from `json.sqlite` (quick.db JSON blobs, see `docs/specs/data-model-legacy.md`) into the normalized SeaORM schema. Runs as the dedicated CLI subcommand defined in ADR 0008. Depends on ADR 0004 (UUIDv7), ADR 0005 (name uniqueness + dedupe), ADR 0006 (no stored score).

## Principles

1. **The JSON blobs are read-only input.** The importer never mutates `json.sqlite`.
2. **All-or-nothing:** the whole import runs in one transaction; any unexpected error rolls back.
3. **Anomalies are reported, not silently fixed or dropped.** Every skip, rename, default-fill and mismatch lands in the import report.
4. **Idempotent by rerun:** importing into a non-empty target either refuses to run or starts from `fresh` — it never merges.

## Algorithm

### Phase 0 — Load & validate

- Load both blobs via typed serde structs (replace the current untyped `serde_json::Value` in `src/legacy/mod.rs`). Unknown fields are logged, not fatal.
- Skip guild entries whose value is not an object (5 known in production) → report as `corrupt_guilds`.

### Phase 1 — Bot stats

Insert legacy counters into `bot_stats` as historical record (moves out of migration `m20251115_000001`, per ADR 0008).

### Phase 2 — Guilds

One row per valid guild: snowflake as `i64`, `is_safe` from `imsafe`. `language` and `permissions` are intentionally not migrated.

### Phase 3 — Trophies (per guild)

1. Iterate the trophies map, skipping the `"current"` key.
2. Apply defaults for the 43 incomplete trophies (creator → 0, created → synthetic timestamp, signed → false, details → default text) → report as `defaulted_fields`.
3. **Name deduplication (ADR 0005):** group the guild's trophies by normalized name (lowercase, non-word characters stripped — the same normalization legacy `getTrophy` used). For every group with more than one member, rename each member to `"{name} {legacy_id}"`. If that exceeds 32 chars, truncate the base name to fit. If the result still collides with any other name in the guild, keep appending until unique. Report every rename as `renamed_trophies` (guild, legacy_id, old name, new name).
4. Generate a **UUIDv7** per trophy in memory; build the mapping `(guild_id, legacy_id) → uuid`.
5. Insert with `legacy_id` column preserved; `created` (Unix ms) converted to timestamp; `image` stored as the opaque string it is (filename or URL); dedication normalized: empty/null shapes → NULL columns, text-only → `dedication_text`, user → `dedication_user_id` (+ name as text).

### Phase 4 — Awards (per guild)

For each user, for each element of the `trophies` array (duplicates included — one row each):

- Mapping hit → insert `user_trophies` row: UUIDv7 id, guild, user `i64`, trophy uuid, `awarded_by = NULL` (legacy never tracked it), synthetic `awarded_at`.
- Mapping miss (orphaned award: trophy deleted in legacy) → drop the row, count per guild → report as `orphaned_awards`.

Users with empty arrays produce no rows (they exist only through their awards; 1,284 such users in production).

### Phase 5 — Rewards, panels, settings

- `rewards` array → `role_rewards` rows (role `i64`, requirement), preserving the legacy sort-by-requirement semantics.
- `panel` → `leaderboard_panels` row (message/channel `i64`). Only ~1 guild has one.
- `settings` → `guild_settings`, storing only keys explicitly present (defaults stay implicit in code, exactly like legacy `getSetting`).

### Phase 6 — Validation & report

- **Counts:** trophies inserted == real legacy count (10,853 expected); awards inserted + orphaned == 60,554 expected; guild count == 2,493 − corrupt.
- **Scores:** per user, compare `SUM(value)` recalculated from the new schema vs legacy `trophyValue`. Mismatches are expected (legacy desync) → report as `score_mismatches` (guild, user, stored, recalculated). Do NOT reconcile (ADR 0006).
- Report is written as JSON + logged summary. Cutover proceeds only after human review of: corrupt_guilds, renamed_trophies, orphaned_awards, score_mismatches counts.

## Images

Files in `./images/` (~2,771) are used as-is; DB stores the legacy filename string. No renaming, no re-hashing. Orphaned files (guild/trophy no longer exists) are left on disk and listed in the report for optional cleanup later.

**Expiring CDN URLs:** 195 production trophies store a full `https://cdn.discordapp.com/...` URL instead of a local filename (mostly written by the `/edit` bug that saved the URL without downloading). Discord CDN URLs are signed and expire since 2024. The importer makes a best-effort download of each URL to a local file (named `{guild_id}_{legacy_trophy_id}.{ext}`) and stores the filename; URLs that are already dead are recorded in the report as `expired_image_urls` and the trophy falls back to no image.

## Cutover runbook (summary)

1. Announce maintenance; stop Node.js bot.
2. Backup: copy `json.sqlite` (timestamped) + export both blobs as JSON.
3. `trophy-bot up` (schema) → `trophy-bot import` (data) → review report.
4. Start Rust bot; smoke-test create/award/leaderboard/show on a test guild.
5. Rollback window 24h: Node.js bot + backup remain deployable untouched.
