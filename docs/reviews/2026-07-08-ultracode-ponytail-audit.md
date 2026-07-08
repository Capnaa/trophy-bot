# Ponytail audit — over-engineering sweep (2026-07-08)

**Scope.** Whole-repo audit for over-engineering / complexity ONLY — dead code,
unused flexibility, hand-rolled stdlib, code duplicating what
Serenity/Poise/SeaORM/serde/clap already provide, single-implementation
abstractions, and logic that fits in fewer lines. Correctness, security and
performance are explicitly out of scope (a separate bug hunt covers those).
`TrophyBot-Copy/` and `docs/archive/` were ignored. `#[cfg(test)]` bodies do not
count toward line-cut totals except where test-only scaffolding leaks into
non-test code or is genuinely redundant.

**Method.** A 10-area sweep (dead code, stdlib re-implementations, native
framework features, YAGNI abstractions, duplication, test scaffolding, config,
manifest, one-caller wrappers, build-then-destructure structs) followed by
adversarial verification of every candidate: grep for every caller before any
`delete`, confirm the named stdlib/framework replacement exists, and re-read the
call sites before proposing a `shrink`. **29 findings raised, 28 confirmed** (one
refuted on verification). Nothing load-bearing is proposed for removal. This is a
report only — no code was changed.

## Findings (ranked, biggest cut first)

- shrink `render::clamp_page` + `PageBounds` struct duplicate the clamp math already in `util::paginate`; `build_view`/`page_slice_ids` clamp then re-derive the slice `util::paginate` already returns. Delete both + their 4 redundant unit tests; `build_view` uses `let (slice, page, last) = util::paginate(&visible, PER_PAGE, requested_page);`. [src/bot/render.rs]  (−30 lines)
- delete Two i18n regression-guard tests (`under_construction_stub_keys_are_gone`, `scaffold_main_ftl_keys_are_gone`) assert deleted keys resolve to themselves — already covered by `missing_key_returns_key`. Delete both. [src/i18n.rs]  (−27 lines)
- yagni `DetailsView` struct + `details_view` fn build three fields the handler immediately destructures into a `CreateEmbed` and nothing else. Inline the three expressions into the embed builder; drop struct + fn. [src/bot/commands/details.rs]  (−25 lines)
- delete stats.rs test module re-implements `fresh_db`/`now`/`seed_guild` byte-identical to `domain::test_support::{fresh_db, insert_guild, now}`. Delete the locals; `use crate::domain::test_support::...` and rename `seed_guild`→`insert_guild`. [src/bot/commands/stats.rs]  (−23 lines)
- yagni `SettingsExport` struct + `From<EffectiveSettings>` are a field-for-field clone with NO transformation, existing only to add `Serialize`. Derive `Serialize` on `EffectiveSettings` and use it directly; JSON shape is byte-identical. [src/bot/commands/export.rs]  (−19 lines)
- shrink Image-attachment validation error-mapping is duplicated verbatim between create and edit (`match images::validate(...)` → ext / UnsupportedType / TooLarge replies). Extract one `validate_or_reply` helper in `images.rs`, call from both handlers. [src/bot/commands/create.rs]  (−18 lines)
- shrink `resolve_dedication` is near-duplicated between create and edit (parse → fetch user name → `dedication_columns`); only the guard differs (create `Option<&str>`, edit `"-"` sentinel). Move one `pub(crate) async fn` into create; each caller keeps its trivial guard at the call site. [src/bot/commands/edit.rs]  (−15 lines)
- yagni `util::reply_error` takes an `ephemeral: bool` that all ~14 callers pass `true`. Drop the param; hardcode `.ephemeral(true)`. [src/bot/util.rs]  (−14 lines)
- delete migrations/tests.rs re-implements `fresh_db`/`now` identical to `test_support` (its `insert_guild` returns a Model, so keep that one). Delete the two; `use crate::domain::test_support::{fresh_db, now};`. [src/migrations/tests.rs]  (−14 lines)
- shrink In `prepare_trophy`, three consecutive `Option<String>`-with-default blocks (details/description/emoji) are structurally identical copy-paste. Replace with one `FnMut` closure `str_or_default(opt, field, default)` called three times; NLL releases the `&mut report` borrow before the later value block. [src/import/mod.rs]  (−11 lines)
- shrink The reward-apply-with-logging tail is copy-pasted across award/revoke/clear (`if let Err(err) = reward_apply::apply_rewards(...) { log::error!("reward application failed after /X ...") }`). Extract one `apply_rewards_logged(ctx, cmd, guild_id, user_id)` helper. [src/bot/commands/award.rs]  (−10 lines)
- shrink award/revoke/clear each build the identical success reply (`CreateEmbed::new().colour(COLOR_SUCCESS).description(desc)` + `reply_embed(ctx, embed, false)`). Add the `util::reply_success(ctx, desc)` helper the prior pass noted was missing. [src/bot/commands/clear.rs]  (−8 lines)
- yagni `EffectiveSettings::get(Setting)` (a 10-line match) has a single caller — the `option_label` loop in settings.rs:286. Fold the match into that loop or index the five fields directly; drop the method. [src/domain/settings.rs]  (−12 lines)
- delete `.cargo/config.toml` sets `rustflags = ["-C", "target-cpu=native"]`, which breaks portable/CI/Docker release builds for no measured win on a Discord I/O-bound bot. Delete the file. [.cargo/config.toml]  (−2 lines)
- native Cargo.toml `[features] default = []` is an empty placeholder ("Features for future expansion" — YAGNI). Remove the block. [Cargo.toml]  (−3 lines)
- shrink Cargo.toml release profile sets three keys equal to their defaults for `lto=true`+`opt-level=3` builds: `debug = false`, `incremental = false`, `debug-assertions = false`. Drop the three redundant lines. [Cargo.toml]  (−3 lines)
- yagni `legacy_connect_options` is a one-line one-(production-)caller wrapper (`ConnectOptions::new(legacy_url(path)).sqlx_logging(false).to_owned()`). Inline it into its single caller in `LegacyDb::open`. [src/legacy/mod.rs]  (−3 lines)
- delete Orphan `commands/` directory at repo root contains only a stray `.DS_Store` (and the untracked `export-985439832388042822.json` dump) — a leftover of the Node.js layout, no code references it. Delete the directory and add `.DS_Store` to `.gitignore`. [commands/]  (−0 lines)

## Verdict

net: -288 lines, -0 deps possible

The dependency set was already verified lean in the prior pass — every crate in
`Cargo.toml` has a live caller, so nothing can be dropped there. The remaining
fat is almost entirely local: duplicated test scaffolding, build-then-destructure
view structs, `bool`/param flexibility no caller exercises, and a handful of
copy-pasted command tails. After these cuts the codebase is lean — the
architecture (domain/render/resolver split, one shared paginate, one shared
leaderboard renderer) is sound; the findings are surface duplication and dead
flexibility, not structural over-engineering.
