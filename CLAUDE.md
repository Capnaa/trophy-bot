# CLAUDE.md

Guidance for Claude Code in this repository.

## What this project is

Trophy Bot is a Discord gamification bot: server admins create custom trophies (name, image, point value) and award them to members; leaderboards and score-threshold role rewards drive engagement. The production bot is Node.js (discord.js v14 + quick.db); this repo hosts its **Rust rewrite** (Serenity 0.12 + Poise 0.6 + SeaORM) together with the legacy source and production data needed for the migration.

## Documentation — source of truth

Validated documentation lives in `docs/` (see `docs/README.md`):

- **`docs/specs/README.md` — complete command index**: every command, what it really does, and its defects. Start here.
- **`docs/specs/rust-parity-plan.md` — master implementation checklist**: command parity table, every defect mapped to its fix (F1–F35), intentional behavior deltas, cutover acceptance criteria. Implementation work tracks against this document.
- `docs/specs/*.md` — per-area functional specs validated against the JS source (claims cited as `file.js:line`), with defects marked **BUG**/**QUIRK** and a "Rust target" section per command. The Rust bot fixes BUGs; it does not reproduce them.
- `docs/specs/data-model-legacy.md` — legacy quick.db structures, verified production statistics, data anomalies.
- `docs/specs/migration-import.md` — legacy → normalized DB import algorithm, validation report, cutover runbook.
- `docs/adr/` — architecture decisions (Rust stack, normalized SeaORM schema, SQLite dev / PostgreSQL prod, UUIDv7 internal IDs, per-guild unique trophy names, computed scores, embedded migration CLI, graceful shutdown).

The legacy JS (`commands/`, `events/`, `globals.js`, `index.js`) remains the reference for behavior questions the specs don't answer. Ignore `TrophyBot-Copy/` entirely (backup). `docs/archive/` holds superseded documents — never use them as a source.

## Rust development rules

- Always run `cargo test` after changing code.
- Use the `log` crate; never `println!`/`eprintln!` in bot code.
- Keep `main` minimal; structure code in modules and functions.
- Schema changes only through SeaORM migrations in `src/migrations/` using the schema API (portable across SQLite and PostgreSQL — no raw SQL).

## Running

- `cargo run` — start the bot. Config via `.env` / env vars: `DISCORD_TOKEN`, `DISCORD_BOT_ID`, `DATABASE_URL` (also `TEST_GUILD_ID` and shard options; `--debug` for verbose logs).
- `cargo run -- up|down|status|fresh|refresh|reset` — embedded schema migrations (no external sea-orm-cli needed).
- Database backend is selected at runtime by the `DATABASE_URL` scheme: `sqlite://` in development, `postgres://` in production.

## Data files

- `json.sqlite` — legacy quick.db production snapshot; read-only input for the importer.
- `bot_db.json` / `guilds_db.json` — the same two JSON blobs exported for inspection.
- `images/` — trophy images named `{guild_id}_{legacy_trophy_id}.{ext}`; filenames are preserved as-is through the migration.
