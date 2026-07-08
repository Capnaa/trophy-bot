# Runbook: first-time data migration (legacy quick.db → new bot)

Operator guide for running the one-shot data migration. The algorithm and its expected numbers live in [../specs/migration-import.md](../specs/migration-import.md); this document is the *how-to*. Works identically for a local dry-run (SQLite) and the production cutover (PostgreSQL) — only `DATABASE_URL` changes.

## Prerequisites

1. **The legacy snapshot**: `json.sqlite` (the quick.db file from the Node.js bot) in the working directory. Take it from the STOPPED legacy bot for the real migration — a live file can be mid-write.
2. **The legacy images**: the `images/` directory from the Node.js bot, next to the working directory. Missing files are tolerated (reported, trophy falls back to no image), but bring what exists.
3. **JSON exports for deep verification** (used by `scripts/verify_import.py`):
   ```bash
   sqlite3 json.sqlite "SELECT json FROM guilds" > guilds_db.json
   sqlite3 json.sqlite "SELECT json FROM bot"    > bot_db.json
   ```
4. **`.env`** with `DATABASE_URL` pointing at the TARGET database (plus `DISCORD_TOKEN`/`DISCORD_BOT_ID` if you'll start the bot afterwards).
5. An **EMPTY target database**. The importer refuses to run into a non-empty target (it never merges). First time = apply the schema fresh.

⚠️ All of `json.sqlite`, `bot_db.json`, `guilds_db.json`, `images/` are git-excluded production data. They exist only where the migration runs.

## Steps (local binary)

```bash
# 1. Backup the source (never touched, but belt and braces)
cp json.sqlite json.sqlite.backup.$(date +%Y%m%d_%H%M%S)

# 2. Apply the schema to the empty target
cargo run -- fresh          # or `up` on a brand-new database

# 3. Run the import (single transaction; seconds, plus best-effort CDN retries)
cargo run -- import --legacy-db ./json.sqlite

# 4. Deep content verification — must print NO PROBLEMS DETECTED
python3 scripts/verify_import.py
```

### Docker variant

`docker-compose.yml` already mounts `./json.sqlite` read-only and the `images/` volume:

```bash
./dev.sh build
./dev.sh migrate    # schema
./dev.sh import     # = docker compose run --rm trophybot import --legacy-db /app/json.sqlite
```

### PostgreSQL (production cutover)

Same steps with `DATABASE_URL=postgres://user:pass@host/dbname` in the environment. Do a full rehearsal against a scratch Postgres BEFORE the real window (pending by design — see the review's residual risks). The cutover sequence around these steps (announce, stop the Node bot, start the Rust bot, 24 h rollback window) is in migration-import.md's runbook.

## Reviewing the result

The import writes `./import-report.json` and logs a summary table comparing every metric against the values measured on the production snapshot — **all 23 rows must say `OK`**. What the reviewed categories mean:

| Report entry | Expected | Meaning / action |
|---|---|---|
| `tombstoned_guilds` | 5 | `/forgetme` deletions — correctly skipped, no action |
| `renamed_trophies` | 643 | duplicate names got their legacy ID suffixed (ADR 0005) — auditable per guild in the JSON |
| `rounded_values` | 44 | float trophy values rounded to integers — listed with before/after |
| `score_mismatches` | 133 (51 `legacy_drift` + 82 `rounding`) | stored legacy scores that were WRONG or float-derived; the recalculated score is correct by definition (ADR 0006) — review, never "fix" |
| `missing_image_files` / `expired_image_urls` | 200 / ≤195 | trophies fall back to no image; expired CDN URLs are only recoverable if run while some still resolve |
| `orphan_disk_files` | 278 | files in `images/` no trophy references — optional manual cleanup |

If ANY row says `MISMATCH` or the verifier reports problems: do not proceed — fix, then `fresh` + re-import (the import is all-or-nothing and repeatable).

## Re-running

The importer is **idempotent by rerun, not by merge**: to repeat it, wipe the target first (`cargo run -- fresh` — this DESTROYS the target database). Never point `fresh` at a database holding post-migration live data.
