# ADR 0006: User score computed on the fly

**Status:** Accepted (2026-07-07)

## Context

Legacy stores a denormalized `trophyValue` per user, incremented/decremented manually on award/revoke/clear. It desynchronizes (failed operations, the revoke.js bug) and there is no recalculation. Global counters in `bot` data are also wrong: 120,411 awards reported vs 60,554 real; 10,571 trophies reported vs 10,853 real.

## Decision

- **No stored score column.** A user's score is always `SUM(trophies.value)` over their `user_trophies` rows via JOIN (single indexed query; leaderboard is the same query grouped by user).
- Legacy `trophyValue` is not migrated. During import it is only used for a validation report: mismatches between stored and recalculated scores are **reported, not reconciled** — the recalculated value is correct by definition.
- Legacy global counters are imported into `bot_stats` as historical record only; live statistics are computed from real data.

## Consequences

- Score can never desync; role rewards and leaderboards always agree.
- If leaderboard queries ever become hot at scale, add caching or a materialized view — not a mutable counter.
