# ADR 0009: Graceful shutdown on SIGINT/SIGTERM

**Status:** Accepted (2026-07-07)

## Context

The bot runs under Docker (`docker stop` sends SIGTERM) and interactively (Ctrl+C sends SIGINT). Killing the process without closing shards leaves the bot appearing online until the gateway times out. Three sibling implementations were compared (this repo — none; serenity-bot — ctrl_c + `shard_manager.shutdown_all()`; agent-launcher — ctrl_c + timed child cleanup).

## Decision

- Spawn the shard runner in a task, keep a `shard_manager` clone, wait on a `shutdown_signal()` future that resolves on **Ctrl+C or (Unix) SIGTERM** via `tokio::select!`, then `shard_manager.shutdown_all().await` and join the runner. Implemented in `src/bot/mod.rs`.
- Release profile must NOT set `panic = "abort"`: a panic in one command handler must stay contained in its tokio task instead of killing the process.

## Consequences

- Clean disconnect: Discord marks the bot offline immediately; in-flight events finish.
- Any future background workers (panel updater, stats) should hook into the same shutdown path.
