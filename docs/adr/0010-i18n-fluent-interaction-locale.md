# ADR 0010: i18n via Fluent, locale from the interaction

**Status:** Accepted (2026-07-08) — implemented in `src/i18n.rs` + `locales/`; usage rules in `docs/specs/i18n.md`

## Context

The legacy bot stored a per-guild `language` field and half-implemented a locale system that was ultimately disabled (`/language` fully commented out); in practice it is English-only. Modern Discord interactions carry the locale directly: `interaction.locale` (invoking user's client language, always present; Poise exposes it as `ctx.locale()`) and `interaction.guild_locale` / `guild.preferred_locale` (server preference). Storing a language setting is no longer necessary.

## Decision

- **Locale resolution:** command replies use `interaction.locale`; non-interaction content (leaderboard panels) uses `guild.preferred_locale`; fallback chain is exact tag → language prefix (`es-ES` → `es`) → `en-US`.
- **No stored language setting.** The legacy `language` field stays unmigrated (already decided in the import spec).
- **Message catalog: Mozilla Fluent** (`fluent` + `fluent-templates`), one `.ftl` file per locale under `locales/`. Fluent handles plurals/interpolation correctly and is the pattern documented for Poise.
- **Phase 1 (cutover):** every user-facing string goes through the translation layer `t(locale, key, args)` from the first command implemented, with a single `en-US` catalog — exact parity with the current English-only bot, no hardcoded strings scattered through code.
- **Phase 2 (post-cutover):** add locales by adding `.ftl` files; also add Discord command `name_localizations`/`description_localizations` at registration.

## Consequences

- Zero translation cost at cutover; adding a language later touches no command code.
- Panels and other background output are per-guild localized, not per-user.
- Command code never formats user-facing text with `format!` directly; review should enforce this.
