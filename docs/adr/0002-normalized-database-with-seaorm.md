# ADR 0002: Normalized database with SeaORM

**Status:** Accepted (2026-07-07)

## Context

quick.db stores ALL data as two JSON blobs in SQLite: table `bot` (global counters) and table `guilds` (a single row holding every guild, trophy, user and award for ~2,500 servers). Every operation deserializes/reserializes JSON by path. There is no integrity, no indexing, no concurrency, and the whole dataset lives in memory.

## Decision

Use **SeaORM** (with `sea-orm-migration`) and a normalized schema. Core tables:

1. `guilds` — Discord server registrations and flags.
2. `trophies` — trophy definitions; keeps `legacy_id` (the old per-guild string ID) for traceability.
3. `user_trophies` — **one row per individual award**. No UNIQUE constraint on `(user_id, trophy_id)`: duplicates are required functionality (verified in production: a user with 2,009 awards over 3 distinct trophies).
4. `guild_settings` — the 5 per-guild settings.
5. `role_rewards` — score-threshold role assignments.
6. `leaderboard_panels` — persistent leaderboard message per guild.
7. `bot_stats` — global counters (historic legacy values imported as-is, new counters maintained correctly).

Foreign keys with `ON DELETE CASCADE`; deleting a trophy removes its awards (replaces the legacy `cleanseTrophies()` sweep).

## Consequences

- O(log n) indexed lookups instead of full-JSON parsing; per-guild isolation; transactions.
- The 150-trophies-per-guild limit becomes a product choice, not a technical necessity.
- Legacy data must be converted (ADR 0008); trophy ID remapping is required (ADR 0004).
- Orphaned awards (user arrays referencing deleted trophies, which the legacy bot tolerates) become impossible; the importer must handle them explicitly.
