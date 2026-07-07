# ADR 0001: Rewrite in Rust with Serenity + Poise

**Status:** Accepted (2026-07-07)

## Context

The production bot is Node.js 18 + discord.js v14 + quick.db. It works, but the codebase has known bugs (see `docs/specs/`), no tests, a database that cannot scale (see ADR 0002), and artificial limits (150 trophies per guild) imposed by the storage design.

## Decision

Rewrite the bot in Rust using:

- **Serenity 0.12** for the Discord API (gateway, HTTP, sharding via `start_shard_range`).
- **Poise 0.6** as the command framework (slash commands, autocomplete, built-in permission checks and cooldowns).
- **Tokio** (multi-threaded) as async runtime.
- **clap v4** for the binary CLI (bot run mode + migration subcommands).
- `log` + `env_logger` for logging (no `println!`/`eprintln!` in bot code).

The rewrite is functionally driven by the validated specs in `docs/specs/`, not by the legacy code structure. Known legacy bugs are fixed, not reproduced (documented per-command in the specs).

Deployment is a single cutover with data migration (see ADR 0008), keeping the Node.js bot ready as rollback for 24h.

## Consequences

- Type safety and compile-time checked queries; predictable memory; one static binary.
- The deprecated custom permission system and the half-implemented language system are NOT reimplemented; Discord native permissions only.
- All 26 slash commands must be reimplemented before cutover; parity is tracked against `docs/specs/`.
