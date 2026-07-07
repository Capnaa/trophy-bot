# Repository Guidelines

Canonical guidance is maintained in [CLAUDE.md](CLAUDE.md) (project overview, development rules, how to run). Validated documentation — command specs, legacy data model, migration plan and architecture decisions — lives under [docs/](docs/README.md); start with the complete command index at [docs/specs/README.md](docs/specs/README.md).

Key rules:

- Run `cargo test` after every code change.
- Use the `log` crate, never `println!`/`eprintln!`; keep `main` minimal and code modular.
- Schema changes only via SeaORM migrations (`src/migrations/`), portable across SQLite (dev) and PostgreSQL (prod).
- The legacy Node.js source (`commands/`, `events/`, `globals.js`) is behavioral reference; `TrophyBot-Copy/` and `docs/archive/` must be ignored.
