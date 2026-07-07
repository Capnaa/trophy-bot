# Trophy Bot documentation

Documentation for the Rust rewrite of Trophy Bot. All content here is validated against the real Node.js source and production data — earlier scattered markdown docs were consolidated into this tree and removed.

## Structure

- **[adr/](adr/README.md)** — Architecture Decision Records: design and architecture decisions for the Rust bot, numbered and immutable once accepted.
- **[specs/](specs/)** — Validated functional specifications: what the legacy bot ACTUALLY does (cited to JS `file:line`), its bugs and quirks, and what the Rust implementation must keep or change.

## Specs

| File | Scope |
|---|---|
| [specs/commands-utility.md](specs/commands-utility.md) | /about /forgetme /help /imsafe /invite /ping /stats /suggest /support (+dead /language) |
| [specs/commands-trophy-management.md](specs/commands-trophy-management.md) | /create /edit /delete /award /revoke /clear /details |
| [specs/commands-admin.md](specs/commands-admin.md) | /export /panel /permissions /rewards /settings |
| [specs/commands-user.md](specs/commands-user.md) | /leaderboard /show /trophies (+dead /trophystats) |
| [specs/core-behaviors.md](specs/core-behaviors.md) | globals.js shared functions, events, dispatch/permission flow, background tasks |
| [specs/data-model-legacy.md](specs/data-model-legacy.md) | quick.db JSON structures, verified production statistics, data anomalies |
| [specs/migration-import.md](specs/migration-import.md) | Legacy → normalized DB import algorithm, validation report, cutover runbook |

## Conventions

- Specs mark legacy defects explicitly as **BUG** (wrong behavior) or **QUIRK** (surprising but arguably intended). The Rust bot fixes BUGs unless an ADR says otherwise.
- Every nontrivial behavioral claim in a spec cites the JS source (`file.js:line`).
- New design decisions go to `adr/` before implementation.
