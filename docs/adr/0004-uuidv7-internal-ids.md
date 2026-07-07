# ADR 0004: UUIDv7 for internal IDs

**Status:** Accepted (2026-07-07)

## Context

Legacy trophy IDs are per-guild string counters ("1".."212", plus a `current` counter key). The first schema draft used `SERIAL` auto-increment. Auto-increment ties ID generation to the database (sequences diverge between the dev SQLite and prod PostgreSQL, and complicate bulk import), and sequential IDs leak creation order.

## Decision

- Primary keys for application entities (notably `trophies`, `user_trophies`) are **UUIDv7**, generated in the application (`uuid` crate, `v7` feature).
- UUIDv7 over UUIDv4: time-ordered, so index locality is good. UUIDv7 over ULID: native `UUID` type in PostgreSQL and first-class support in SeaORM/sqlx (`with-uuid`).
- **UUIDs are internal only and never user-facing.** Users identify trophies by name (ADR 0005). Discord IDs (guilds, users, channels, messages, roles) remain what Discord issues: snowflakes stored as `i64`.
- `trophies.legacy_id` keeps the old per-guild string ID for traceability and for the importer's mapping.

## Consequences

- The importer generates the full `(guild, legacy_id) → uuid` mapping in memory before inserting, with no DB round-trips for ID discovery.
- In SQLite, UUIDs are stored as BLOB/TEXT (SeaORM abstracts this); tests that inspect raw rows must account for it.
- Embeds/footers no longer show a numeric "Trophy ID"; display uses the trophy name.
