# ADR 0003: SQLite in development, PostgreSQL in production

**Status:** Accepted (2026-07-07)

## Context

Local development should not require running a database server. Production needs concurrency, proper types and operational tooling.

## Decision

- Both backends enabled at compile time via SeaORM features `sqlx-sqlite` + `sqlx-postgres` (`runtime-tokio-rustls`).
- The backend is selected at runtime by the `DATABASE_URL` scheme (`sqlite://...` vs `postgres://...`). No recompilation, no cargo feature switching.
- Schema is defined exclusively through SeaORM's migration API (Rust), never raw SQL, so the same migration runs on both engines.

## Consequences

- Dev/prod parity caveats to keep in mind: SQLite stores UUIDs as BLOB/TEXT (no native type), has weaker typing and different locking. Anything migration-critical must also be tested against a local PostgreSQL (Docker) before production cutover.
- Raw SQL in code must stay portable or be avoided in favor of sea-query builders.
