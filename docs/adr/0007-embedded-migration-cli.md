# ADR 0007: Migrations embedded in the bot CLI

**Status:** Accepted (2026-07-07)

## Context

Schema migrations could be run with the external `sea-orm-cli` tool, but that requires installing it wherever the bot deploys. An earlier iteration depended on the `sea-orm-cli` crate as a library to reuse its `MigrateSubcommands`, which duplicated an enum and desynchronized RC versions.

## Decision

- The bot binary itself exposes migration subcommands: `trophy-bot up|down|status|fresh|refresh|reset` (see `src/cli.rs`, `src/migrations/mod.rs`). Running with no subcommand starts the bot.
- Implemented by calling `MigratorTrait` methods (`Migrator::up`, etc.) directly from our own clap enum — **no dependency on the `sea-orm-cli` crate**.
- Migration commands log at INFO level even when the bot's default level is WARN; sqlx query noise only with `--debug`.
- Scaffolding (`migrate generate`) and entity codegen (`generate entity`) are dev-time tasks; migration files are written by hand (or with a locally installed `sea-orm-cli` if ever needed).

## Consequences

- One deployable artifact handles both serving and schema management.
- Schema migrations create/alter tables only; they must not depend on legacy data files (see ADR 0008).
