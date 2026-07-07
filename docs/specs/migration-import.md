# Spec: Legacy data import

How production data moves from `json.sqlite` (quick.db JSON blobs, see `docs/specs/data-model-legacy.md`) into the normalized SeaORM schema (`docs/specs/schema.md`). Runs as the dedicated CLI subcommand defined in ADR 0008. Depends on ADR 0004 (UUIDv7), ADR 0005 (name uniqueness + Unicode-aware dedupe), ADR 0006 (no stored score). All expected counts below were measured against the real production snapshot.

## Principles

1. **The JSON blobs are read-only input.** The importer never mutates `json.sqlite`.
2. **All-or-nothing:** the whole import runs in one transaction; any unexpected error rolls back.
3. **Anomalies are reported, not silently fixed or dropped.** Every skip, rename, rounding, default-fill and mismatch lands in the import report.
4. **Idempotent by rerun:** importing into a non-empty target either refuses to run or starts from `fresh` — it never merges.

## Algorithm

### Phase 0 — Load & validate

- Load both blobs via typed serde structs (replace the untyped `serde_json::Value` in `src/legacy/mod.rs`). Structs must tolerate unknown fields (**no `deny_unknown_fields`**); the ubiquitous vestigial keys `id`, `restapi`, `language` and the one-off typo `tropies` are expected and not logged as noise.
- Guild entries whose value is the literal integer `-1` are `/forgetme` **tombstones** (5 in production): deliberately deleted data. Skip → report `tombstoned_guilds`. Any other non-object guild value → report `corrupt_guilds` (0 expected).

### Phase 1 — Bot stats

Insert into `bot_stats` as historical record: the per-command counters (`commands.*`), `trophies`, `trophiesAwarded`. Explicitly NOT migrated: `version`, `defaultLanguage`, `bannedUsers` (empty), `lastDay`, `milestone`. (Moves the insert out of migration `m20251115_000001`, per ADR 0008.)

### Phase 2 — Guilds

One row per valid guild (2,488 expected): snowflake as `i64`, `is_safe` from `imsafe` (**absent → false**; 81 guilds). NOT migrated: `language`, `permissions`, `restapi`, `id`.

### Phase 3 — Trophies (per guild; 10,853 expected)

1. Iterate the trophies map, skipping the `"current"` key.
2. Apply defaults for the 43 incomplete trophies (creator → NULL, created → synthetic timestamp, signed → false, details → default text) → report `defaulted_fields`.
3. **Value normalization:** 44 trophies have non-integer float values (e.g. 8.5). The `value` column is INTEGER: round half-away-from-zero → report `rounded_values` (guild, legacy_id, original, rounded).
4. **Name deduplication (ADR 0005):** compute each trophy's normalized name (Unicode-aware: lowercase, keep alphanumeric of any script; if empty — emoji-only — fall back to the lowercased raw name). Group by that key; every member of a group with >1 entries is renamed to `"{name} {legacy_id}"`, truncating the base name if the result exceeds 32 chars (21 cases expected), re-disambiguating on residual collision. Expected: **286 groups / 641 renames / 209 guilds** → report `renamed_trophies`.
5. Generate a **UUIDv7** per trophy in memory; build the mapping `(guild_id, legacy_id) → uuid`.
6. Insert with `legacy_id` preserved and `normalized_name` computed; `created` (Unix ms) → timestamp; dedication normalized (empty/null shapes → NULLs; text-only → `dedication_text`; user → `dedication_user_id` + name text); image handled in the Images phase below.

### Phase 4 — Awards (per guild; 60,554 elements expected)

For each user, for each element of the `trophies` array (duplicates included — one row each):

- Mapping hit → insert `user_trophies` row: UUIDv7 id, guild, user `i64`, trophy uuid, `awarded_by = NULL` (legacy never tracked it), synthetic `awarded_at`.
- Mapping miss → drop + report `orphaned_awards`. **Production currently has 0 orphans**; the path exists as defense, and validation expects `inserted == 60,554, orphaned == 0`.

Users with empty arrays (1,284) produce no rows.

### Phase 5 — Rewards, panels, settings

- `rewards` → `role_rewards` rows. **Dedupe duplicate role IDs first** (7 guilds repeat a role 2–3 times): keep the entry with the LOWEST requirement, and report `deduped_rewards`. Note this is a deliberate FIX, not legacy parity: legacy `doRewardRoles` applied role additions before removals, so a duplicated role landed in both lists and was effectively ALWAYS stripped (suppression bug, see commands-admin.md) — keeping the lowest requirement is the user-favorable reading of the admin's intent. 287 → ~280 rows expected; `UNIQUE(guild_id, role_id)` then holds.
- `panel` → `leaderboard_panels` (**461 rows expected**, all `{message, channel}` digits). Many targets will be stale; the post-cutover reconciliation sweep (parity F32) must expect a large initial cleanup, so panel message/channel validity is NOT checked at import.
- `settings` → `guild_settings`, storing only keys explicitly present (162 guilds non-empty; all values verified in range). Missing keys stay NULL → code-side defaults, exactly like legacy `getSetting`.

### Phase 6 — Images

DB stores the filename string; files in `./images/` are used as-is (no renaming — filenames keep legacy trophy IDs).

- **Local filenames (2,693 refs):** if the file exists on disk, store the filename. **200 referenced files are missing from disk** → store NULL + report `missing_image_files`. ~60 files have non-image extensions (webp/mp4/exe/...) — stored as-is; the bot serves them or falls back gracefully (never crashes, parity F17).
- **CDN URLs (195, all cdn.discordapp.com, signed and expiring):** best-effort download to `{guild_id}_{legacy_trophy_id}.{ext}` and store the filename; dead URLs → NULL + report `expired_image_urls`.
- **Orphan disk files (278, of which 124 have `?ex=...` query strings baked into the filename):** left on disk, listed in the report for optional manual cleanup.

### Phase 7 — Validation & report

- **Counts:** valid guilds == 2,488; tombstones == 5; trophies == 10,853; awards == 60,554 with orphans == 0; renames == 641; rounded values == 44; panels == 461; rewards ≈ 280 after dedupe.
- **Scores:** per user, compare stored `trophyValue` vs recalculated `SUM(value)` — **float-tolerant** comparison (60 users have float stored values). Expected mismatches: **54 users** (drift −990..+3,500) → report `score_mismatches`; NOT reconciled (ADR 0006) — the recalculated value is correct by definition.
- Report written as JSON + logged summary. Cutover proceeds only after human review of: tombstoned/corrupt guilds, renames, rounded values, missing/expired images, deduped rewards, score mismatches.

## Staging: local-first validation

Current focus is (a) migrating the code and (b) leaving the data import ready and PROVEN. All import development and validation happens **locally against a copy of `json.sqlite`, targeting SQLite**, until the importer completes a full single-shot run whose report matches every expected count above. PostgreSQL portability is entrusted to SeaORM's engine-agnostic schema/query API (ADR 0003) and is deliberately NOT validated now — a Postgres validation run happens only pre-cutover, once command parity (rust-parity-plan.md) is complete. Nothing in this phase touches production. Ordered implementation steps: `docs/specs/implementation-plan.md`.

```bash
cp json.sqlite test_import_source.sqlite   # read-only input copy
cargo run -- fresh                          # clean local target schema (DATABASE_URL=sqlite://...)
cargo run -- import                         # full import + report
```

## Cutover runbook (summary — DEFERRED, not current scope)

1. Announce maintenance; stop Node.js bot.
2. Backup: copy `json.sqlite` (timestamped) + export both blobs as JSON.
3. `trophy-bot up` (schema) → `trophy-bot import` (data) → review report against the expected counts above.
4. Start Rust bot; smoke-test create/award/leaderboard/show on a test guild. Note: in guilds where the bot has Administrator, legacy role rewards were LIVE (see core-behaviors.md) — the first score change per user recomputes their reward roles idempotently; expect visible role adjustments mainly in guilds where the feature was dead.
5. Rollback window 24h: Node.js bot + backup remain deployable untouched.
