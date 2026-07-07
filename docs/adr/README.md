# Architecture Decision Records

Design and architecture decisions for the Rust rewrite of Trophy Bot. Each ADR is immutable once accepted; superseding decisions get a new ADR that references the old one.

Format: Status / Context / Decision / Consequences.

## Index

- [0001 - Rewrite in Rust with Serenity + Poise](0001-rust-rewrite-with-serenity-poise.md)
- [0002 - Normalized database with SeaORM](0002-normalized-database-with-seaorm.md)
- [0003 - SQLite in development, PostgreSQL in production](0003-sqlite-dev-postgresql-prod.md)
- [0004 - UUIDv7 for internal IDs](0004-uuidv7-internal-ids.md)
- [0005 - Trophy names unique per guild](0005-trophy-names-unique-per-guild.md)
- [0006 - User score computed on the fly](0006-score-computed-on-the-fly.md)
- [0007 - Migrations embedded in the bot CLI](0007-embedded-migration-cli.md)
- [0008 - Legacy data import as a dedicated CLI subcommand](0008-legacy-import-as-cli-subcommand.md)
- [0009 - Graceful shutdown on SIGINT/SIGTERM](0009-graceful-shutdown.md)
- [0010 - i18n via Fluent, locale from the interaction](0010-i18n-fluent-interaction-locale.md)
