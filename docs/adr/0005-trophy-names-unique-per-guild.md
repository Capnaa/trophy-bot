# ADR 0005: Trophy names unique per guild

**Status:** Accepted (2026-07-07)

## Context

With internal UUIDs (ADR 0004), users need a human identifier for commands (`/award trophy:<...>`). The natural one is the trophy **name**. The legacy bot never enforced name uniqueness and resolved names to "first match", which is ambiguous. Verified against production data (2026-07-07): **176 of 2,493 guilds (7%) contain exact duplicate trophy names** (198 guilds if compared case/whitespace-insensitively); e.g. one guild has three trophies named "Do You Smell Barbecue?". There is no way to tell duplicates apart in a command (`trophy:5` vs `trophy:5`).

## Decision

- `UNIQUE(guild_id, name)` constraint on `trophies`. Names may repeat freely across different guilds.
- Commands resolve trophies by name within the guild, with slash-command autocomplete (the autocomplete choice displays the name and can carry the UUID as value).
- **Legacy deduplication rule** (applied by the importer): within a guild, trophies sharing the same name each get the legacy numeric ID appended with a space — `"test"` (id 3) and `"test"` (id 7) become `"test 3"` and `"test 7"`. Same-ID duplicates cannot exist (JSON object keys are unique per guild). If appending exceeds the 32-char name limit, the base name is truncated to fit. If the result still collides with another existing name in the guild, the importer keeps appending disambiguators until unique. Every rename is recorded in the import report.
- Uniqueness is **case- and punctuation-insensitive** via an app-maintained `normalized_name` column with a `UNIQUE(guild_id, normalized_name)` index (a portable solution across SQLite/PostgreSQL; a `lower()` functional index cannot express the full rule).
- **Normalization is Unicode-aware**: lowercase (Unicode), keep only alphanumeric characters *of any script* (Rust `char::is_alphanumeric`). The legacy `getTrophy` used JS `\W`, which strips ALL non-ASCII — that would have falsely grouped 155 distinct Cyrillic/CJK trophies in 18 guilds (one guild has 62 distinct Chinese-named trophies) as "duplicates". Validated legacy behavior still motivates insensitivity: differently-cased names already collided in legacy resolution.
- **Empty-normalization fallback**: names that normalize to empty (emoji/symbol-only — 17 in production) use the lowercased raw name as their normalized key, so distinct emoji names stay distinct and exact emoji duplicates (2 in production) still collide.
- Measured with this rule against production: **286 duplicate groups, 641 trophies to rename, in 209 guilds**; 21 renames need base-name truncation to fit 32 chars.

## Consequences

- Ambiguity disappears permanently; `/create` and `/edit` must validate name availability.
- 641 historical trophies across 209 guilds get renamed at import; guild admins may notice — the import report per guild makes this auditable.
