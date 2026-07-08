# Migration review verdict — Node.js → Rust rewrite

**Date:** 2026-07-08
**Scope:** Full-tree review of the Rust bot (`src/`, Serenity 0.12 + Poise 0.6 + SeaORM rc.42) against the legacy Node.js bot (`commands/`, `events/`, `globals.js`) and the validated specs in `docs/specs/`.
**Out of scope (by instruction):** `src/smoke/` and `src/cli.rs` (concurrent smoke-subcommand development), `TrophyBot-Copy/`, `docs/archive/`. No `cargo run` (would connect to Discord); nothing committed by this review.

---

## 1. Executive verdict

**The Rust bot is feature-equivalent to the Node.js bot and is ready for the pre-cutover phase.** Every divergence from legacy behavior is either a fix from the F1–F37 catalog (`rust-parity-plan.md` §3), a cross-cutting §2 mandate, or a documented intentional delta in §4. The importer is proven against the real production data. The codebase is maintainable: modular, i18n-complete, tested, and clippy-clean.

### Evidence

| Dimension | State |
|---|---|
| Test suite (`cargo test --all-targets`, final run 2026-07-08) | **400 passed, 0 failed, 0 ignored** |
| Lints (`cargo clippy --all-targets`) | Clean — no warnings, no errors |
| Fix catalog | All **37 entries (F1–F37)** in `rust-parity-plan.md` §3 resolved; cross-cutting §2 items (success-only run counters, defensive runtime permission checks, ephemeral error replies, i18n-only user strings) implemented and test-locked |
| Intentional deltas | All behavior changes documented in `rust-parity-plan.md` §4 (incl. §4.8 /forgetme button expiry, §4.9 /delete confirmation, §4.10 /rewards remove autocomplete) |
| Importer | **Gate-verified against real production data** (local single-shot run, SQLite target per `migration-import.md`): 2,488 valid guilds + 5 tombstones; 10,853 trophies; 60,554 awards with **0 orphans**; 643 renames; 44 rounded float values; 461 panels; 275 rewards after dedupe (12 removed); 133 score mismatches all classified `legacy_drift`/`rounding` (not reconciled by design, ADR 0006) |
| Review sweep dimensions | Command-family parity vs legacy source (utility, trophy-management, admin, user — one pass per spec file), core behaviors/bot infrastructure (dispatch, hooks, shutdown, logging), importer/data model, i18n catalog coverage |
| Review yield | **43 findings raised, 38 confirmed, 5 refuted** — all 38 confirmed findings resolved (fixed, or spec-documented as intentional; see §2) |
| Rust house rules | `log` crate only (no println/eprintln in `src/bot`), all user-facing strings via Fluent (`locales/en-US/*.ftl`) with catalog-key regression tests, no hanging-interaction paths (early shard-runner exit now triggers full shutdown), no process-killing paths |

Note: a concurrent session landed related batches (paginate move, /delete confirmation button, rewards autocomplete, part of the counters fix) into the same tree during remediation; the final green run above covers the **merged** tree at `b8402a1`.

---

## 2. Findings

**Totals:** 43 raised → **38 confirmed**, **5 refuted** during verification. Resolutions below are taken from the final remediation fix reports and verified in the current tree; earlier-batch confirmations were absorbed into the F1–F37 implementation work and are covered by the acceptance evidence in §1. Where two review passes surfaced the same defect it is listed once with both angles noted.

| # | Finding (severity) | Resolution |
|---|---|---|
| 1 | Command run counters recorded only by `/stats` itself — no `post_command` hook, so "Runs" and per-command counters froze for every other command, violating §2 "count successful executions only" (**error**; raised independently by two passes) | **Fixed.** `FrameworkOptions.post_command` wired in `src/bot/mod.rs` (`record_command_run` → `record_run_counters` → `stats::record_successful_run`). Poise 0.6.2 invokes `post_command` only after the command action returns `Ok`, so counters are success-only. `root_command_name` keys counters by the top-level command (legacy parity: `/rewards add` counts under `rewards`, matching imported legacy rows). Counter failures are logged and swallowed so an already-answered command never errors. `/stats` no longer self-increments. Tests: `record_run_counters_bumps_total_and_per_command`, `record_run_counters_swallows_database_errors`, `run_counters_are_keyed_by_top_level_command_name`. |
| 2 | No defensive runtime permission check — all management commands relied solely on `default_member_permissions`; the `MissingUserPermissions` error arm was dead code and Integrations overrides could expose destructive commands (**error/minor**; raised by two passes) | **Fixed.** `required_permissions = "MANAGE_GUILD"` added to all 12 Manage Guild commands (award, revoke, delete, create, edit, clear, details, imsafe, permissions, panel, rewards, settings) and `"ADMINISTRATOR"` to export and forgetme, mirroring each `default_member_permissions`. Poise checks parent commands before leaves, so parent-level declarations cover all subcommands. Tests extended to lock the declarations. **Accepted trade-off** (flagged by verifier): members granted access purely via Integrations overrides without holding Manage Guild are now refused with the localized missing-permissions embed — exactly what §2's defensive mandate requires. |
| 3 | Startup command-listing task discarded its `JoinHandle`, silently dropping HTTP errors; plus warn-level log spam for routine per-command listing (**minor**; two passes, same defect) | **Fixed** (one change covers both). The task logs success as a single `log::info!` line via the testable `format_command_listing` helper and logs failures with `log::error!` inside the task; per-command warn-spam removed, "Sync commands in test guild" downgraded warn→info. Handle stays deliberately detached (one-shot, read-only, harmless at shutdown). Test: `command_listing_formats_names_and_empty_case`. |
| 4 | Zero tests for `handle_framework_error` i18n keys (and `cli.rs`) — a renamed Fluent key would ship raw key ids to users (**minor**) | **Fixed for `src/bot/mod.rs`:** `framework_error_messages_exist_in_catalog` asserts common-error-title/-generic/-missing-user-permissions/-guild-only/-not-owner/-invalid-input resolve to real catalog entries; `framework_error_messages_interpolate_arguments` exercises both cooldown plural branches and the bot-permissions interpolation. **`cli.rs` half deliberately not addressed** — out of scope due to concurrent smoke-subcommand development in that file (see §3). |
| 5 | `Bot::run` hung forever if the shard runner exited early (invalid token, gateway boot failure): the process sat in signal-wait while the panel updater swept a dead bot (**error**) | **Fixed.** New `until_shutdown_or_runner_exit` helper (`tokio::select!` over the shutdown signal and the runner `JoinHandle`); an early runner exit triggers the full ADR 0009 shutdown path (log, `panel_shutdown` flag, `shutdown_all()`, join panel task) then propagates the error so a supervisor restart fires. Bonus hardening: a failed signal-listener installation is treated as a shutdown request so cleanup still runs. Tests: `shutdown_signal_wins_while_runner_is_alive`, `early_runner_exit_is_detected_without_a_signal`, `failed_signal_listener_is_treated_as_shutdown`. |
| 6 | `/forgetme` confirmation buttons expire after 60 s — user-visible divergence (legacy button lived forever) sanctioned by neither F33 nor §4 at the time of review (**minor**, behavior-divergence) | **Resolved by documentation.** Now an explicit intentional delta: `rust-parity-plan.md` §4.8. Deliberate safety improvement for an irreversible action; the window is announced in the warning embed and the stateless timestamped custom-id survives restarts. |
| 7 | `/delete` shipped without the confirmation button the spec's Rust target requires, deferred via a stale "blocked on C16" TODO (**minor**, deferred-spec-item; second pass also flagged the stale TODO as an error) | **Fixed.** The confirmation flow shipped on the C16 button infrastructure (`src/bot/buttons.rs`): warning embed states how many awards the cascade will remove, invoker-only Confirm/Cancel pair, 60 s expiry, deletion + image cleanup + reward recompute on confirm. Stale TODO removed; documented in §4.9 and the module docs of `src/bot/commands/delete.rs`. |
| 8 | Dedication fetch-failure parity: an unresolvable mention/snowflake was stored as a USER dedication (`Some(id)`, text `NULL`), rendering a broken `<@…>` mention — legacy `parseUser` fell back to a TEXT dedication with the typed input (**minor**) | **Fixed.** New pure helper `dedication_columns` in `src/bot/commands/create.rs`, shared by `resolve_dedication` in create and edit: an unresolvable id now stores `(None, Some(raw_text))`, exactly like legacy. Tests: `resolved_user_dedication_stores_id_and_name_snapshot`, `unresolvable_user_dedication_falls_back_to_the_raw_text`, `text_dedication_ignores_any_fetched_name`. |
| 9 | `/create` and `/edit` error replies issued after the public `ctx.defer()` on the image path requested `ephemeral(true)` but rendered as public followups (download-failure and race-duplicate paths), violating §2 "all error replies ephemeral" (**minor**) | **Fixed.** New `util::reply_error_ephemeral` in `src/bot/util.rs`, used by both commands' post-defer error paths so the error is delivered privately without leaving the public deferred response dangling. |
| 10–38 | Remaining confirmed findings from the per-dimension sweeps (command parity details across the four command families, importer edge cases, i18n coverage, infrastructure) | Resolved in the earlier remediation batches folded into the F1–F37 implementation and the concurrent session's commits (final state `b8402a1`); all are covered by the 400-test green suite and the §4 delta list. No confirmed finding remains open. |

**Refuted: 5** — findings that did not survive verification (claimed defects where the code, poise's actual dispatch semantics, or the spec's sanctioned deltas contradicted the raised failure scenario). No action was required for these.

---

## 3. Residual risks & recommendations before cutover

1. **PostgreSQL validation is still pending — by design.** All importer development and gate verification ran locally against SQLite (`migration-import.md`, "Staging: local-first validation"). Postgres portability is entrusted to SeaORM's engine-agnostic API (ADR 0003) but **must be validated with a full schema + import + report run against a real PostgreSQL instance before cutover**, comparing the report to the same expected counts.
2. **Manual Discord smoke test recommended.** This review could not run the live bot (`cargo run` connects to Discord). A smoke harness exists (`trophy-bot smoke`, commit `940cf87`), but before cutover run the runbook's step 4 on a test guild: create → award → leaderboard → show → delete-confirm → forgetme-cancel, plus one framework-error path (missing permissions) to see the localized embeds live.
3. **Permission-check trade-off to announce.** With `required_permissions` enforced, members granted command access solely via Server Settings → Integrations overrides but lacking Manage Guild are now refused. This is the §2 defensive mandate working as intended, but it changes behavior for guilds using delegation — mention it in the cutover announcement.
4. **Deliberately rejected / out-of-scope items.**
   - `cli.rs` test coverage was **not** added (concurrent smoke-subcommand development made the file off-limits); add CLI arg-parsing tests once that work lands.
   - The startup command-listing `JoinHandle` remains deliberately detached (one-shot, read-only); the ADR 0009 hook sub-claim was judged immaterial.
5. **Behavioral deltas users will notice.** 60 s expiry on /forgetme and /delete confirmations (§4.8–4.9) and the /delete confirmation step itself; /rewards remove now uses autocomplete (§4.10). All documented; include in release notes.
6. **Import-report human review is a hard gate.** The 133 score mismatches (51 legacy drift, 82 rounding-induced) are intentionally not reconciled (ADR 0006 — recomputed score is correct by definition); the cutover runbook requires human sign-off on tombstones, renames, rounded values, images, deduped rewards, and mismatch classification before starting the Rust bot.
7. **Role-reward recompute on first score change.** In guilds where the bot has Administrator, legacy rewards were live; elsewhere the feature was effectively dead. Expect visible (idempotent) role adjustments after cutover, mainly in the latter guilds (runbook step 4 note).
8. **Keep the rollback path warm.** Node.js bot + timestamped `json.sqlite` backup stay deployable for the 24 h rollback window per the runbook.

---

*Review executed under the constraint set: no `cargo run`, no commits, `src/smoke/`//`src/cli.rs` untouched. Final verification: `cargo test --all-targets` → 400/400 green; `cargo clippy --all-targets` → clean.*
