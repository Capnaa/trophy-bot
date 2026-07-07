# ADR 0008: Legacy data import as a dedicated CLI subcommand

**Status:** Proposed (2026-07-07)

## Context

Legacy production data lives in `json.sqlite` (quick.db). The first migration (`m20251115_000001`) mixed schema creation with importing legacy bot stats, silently skipping the import when `json.sqlite` is absent. That makes migrations non-deterministic across environments and unrepeatable.

## Decision

- SeaORM migrations define **schema only**.
- Legacy import is a separate subcommand (e.g. `trophy-bot import --legacy-db sqlite://json.sqlite`), runnable and re-runnable independently, that:
  1. Loads and validates both legacy JSON blobs (typed structs, not raw `Value`).
  2. Runs the full conversion in one transaction (algorithm in `docs/specs/migration-import.md`).
  3. Emits a machine-readable + human-readable **import report** (counts, renames, skipped corrupt entries, score mismatches, orphaned awards).
- The existing legacy-stats insert inside `m20251115_000001` moves into this subcommand.

## Consequences

- `fresh`/`refresh` in dev works without production data present.
- The cutover runbook becomes: backup → `up` (schema) → `import` (data) → validate report → start bot.
