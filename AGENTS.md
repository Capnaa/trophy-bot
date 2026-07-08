# Repository Guidelines

Canonical guidance is maintained in [CLAUDE.md](CLAUDE.md) (project overview, development rules, how to run). Validated documentation — command specs, legacy data model, migration plan and architecture decisions — lives under [docs/](docs/README.md); start with the complete command index at [docs/specs/README.md](docs/specs/README.md).

Key rules:

- Run `cargo test` after every code change.
- Use the `log` crate, never `println!`/`eprintln!`; keep `main` minimal and code modular.
- Schema changes only via SeaORM migrations (`src/migrations/`), portable across SQLite (dev) and PostgreSQL (prod).
- The legacy Node.js source lives only under `TrophyBot-Copy/` (behavioral reference; spec citations map to its layout); `docs/archive/` must never be used as a source.
- `json.sqlite`, `bot_db.json`, `guilds_db.json` and `images/` are git-excluded production data: only the importer uses them; regular tests must not depend on them (`cargo test -- --ignored` for snapshot validations).
