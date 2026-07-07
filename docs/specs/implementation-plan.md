# Spec: Implementation plan (code migration)

Detailed, ordered plan for migrating the bot code from Node.js to Rust. Scope per current staging: everything runs and is validated **locally on SQLite** — PostgreSQL portability is entrusted to SeaORM's schema API (ADR 0003) and will be validated only pre-cutover. Cutover itself is out of scope (see migration-import.md).

Each item lists the F-fixes it must implement (rust-parity-plan.md) and its verification. Standing rules for every step: `cargo test` green, strings via i18n (`docs/specs/i18n.md`), `log` crate only, behavior tracked against the command's spec.

## Phase 0 — Pre-cleanup

- Remove the debug legacy dump from `src/main.rs` (println of guild JSON — violates the logging rule; `main` goes back to: migrate subcommand or run bot).
- Remove the demo `bench` command when the first real command lands.

## Phase 1 — Schema & shared layer

1. **Initial migration rewritten** implementing exactly `docs/specs/schema.md`: guilds, trophies, user_trophies, guild_settings, role_rewards, leaderboard_panels, bot_stats. Verify: `fresh` + `status` on SQLite; inspect created DDL.
2. **SeaORM entities** (`src/entities/`) matching the migration.
3. **Shared domain layer** (`src/domain/` or similar), the pieces every command reuses:
   - `score(guild, user)` and `leaderboard(guild, page)` — the SUM/GROUP BY queries (ADR 0006).
   - Trophy resolver: exact `normalized_name` lookup + the Unicode normalization function (ADR 0005; property tests incl. emoji-only and non-Latin names).
   - Settings reader with NULL→default semantics (2/1/1/0/0).
   - Reward engine: compute target role set from score + `stack_roles`, diff, apply awaited and idempotently (F21-F24 groundwork; §2 crash-safety).
   - Error/reply helpers: ephemeral support, error embeds, every handler wrapped (§2).

## Phase 2 — Importer (`trophy-bot import`)

4. **Typed legacy loader**: serde structs replacing `serde_json::Value` (tolerates `restapi`/`id`/`language`/`tropies`; `-1` tombstones as enum variant).
5. **Import subcommand**, phases 0-7 of migration-import.md in one transaction + JSON report.
6. **Full local run** against the real `json.sqlite` copy. Gate: report matches EXACTLY — 2,488 guilds / 5 tombstones / 10,853 trophies / 44 rounded values / 643 renames / 60,554 awards, 0 orphans / 275 rewards after dedupe / 461 panels / 133 score mismatches (51 legacy drift + 82 rounding-induced) / 200 missing + 195 downloaded-or-expired images. This proves the single-shot migration.

## Phase 3 — Commands (implementation order)

| # | Command | Work & fixes | Depends on |
|---|---|---|---|
| C0 | `/ping`, `/about` | Bootstrap the real command pattern: poise registration, i18n, wrapped errors, ephemeral. `/ping` measures real gateway ping + round trip (F35) | Phase 1 |
| C1 | `/create` | Validate-then-persist (F3), integer value (F4), name-uniqueness with normalized check (F5), image pipeline: download+validate type/1MB, store filename | shared layer |
| C2 | `/show` | Trophy **autocomplete + resolver** shared infra (F12) born here; image local/URL/default with graceful fallback (F17); dedication display incl. fixed mode 1 (F36) | C1 |
| C3 | `/award` | Count 1-50 strictly (F8), `awarded_by` recorded (F9), score via SUM, reward engine invoked (awaited) | C2 resolver, reward engine |
| C4 | `/trophies user`, `/trophies guild` | Aggregation ×N, orphans impossible (F20), fallback names (F18), manager exemption implemented as intended (F19) | score/leaderboard queries |
| C5 | `/leaderboard` | Shared renderer (reused by panels): clamped pages (F14), zero-score users shown + real total (F15), departed-user fallback, never crash (F13), deterministic membership check (F16) | C4 queries |
| C6 | `/revoke` | Remove N of the REQUESTED trophy, most recent first (F1); honest feedback (F2); reward recompute | C3 |
| C7 | `/clear` | Full reset + reward recompute | C3 |
| C8 | `/edit` | Image pipeline reuse (F6), value/signed immutable documented (F7), accurate change report (F37) | C1 |
| C9 | `/delete` | Hard delete + FK cascade, reward recompute for affected users, safe image unlink (F10) | C3 |
| C10 | `/details` | Ephemeral reply, Manage Guild enforced (F11) | C2 resolver |
| C11 | `/settings set/list` | Native Discord choices (F26), typed setting enum (F27) | shared settings |
| C12 | `/rewards add/remove/clear/list` | Real hierarchy check (F21), no duplicate roles (F22), exactly 20 (F23), removable deleted roles + marked in list (F24), correct descriptions (F25), explicit empty state | reward engine |
| C13 | `/panel create/delete` + background updater | Event-driven refresh with debounce + low-frequency reconciliation sweep (F29, F32), old message cleanup on create/delete (F30), transactional record (F31); hooks into graceful shutdown (ADR 0009). Day-one: reconcile the 461 imported panels | C5 renderer |
| C14 | `/stats` | Live counts from DB (F34), success-only counters, real 10s cooldown | bot_stats |
| C15 | `/export` | Ephemeral, normalized versioned JSON (F28) | entities |
| C16 | `/forgetme` | Owner-only + confirmation button flow, TRUE cascade delete + image file removal, then leave (F33) | button infra |
| C17 | `/imsafe`, `/permissions`, `/help`, `/invite`, `/support`, `/suggest` | Statics: imsafe no-op confirmation; permissions deprecation notice; help content rewritten (no legacy permissions lesson); invite/support truly ephemeral | C0 pattern |

Not implemented (never functional in legacy): `/language`, `/trophystats`.

Per command: unit/integration tests covering its F-items; manual check on the test guild (`TEST_GUILD_ID`) against its spec section.

## Phase 4 — Local integration validation

7. Fresh DB → `import` → bot on test guild with real imported data.
8. Smoke flow: create → award ×N → leaderboard → revoke (correct trophy) → role applied → clear → panel updates.
9. Full pass over rust-parity-plan.md §6 items 2-3.

## Deferred (pre-cutover, explicitly not now)

- PostgreSQL validation run (same import + smoke against local Postgres).
- Cutover runbook (migration-import.md) + 24h rollback window.
