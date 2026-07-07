# ADR 0005: Trophy names unique per guild

**Status:** Accepted (2026-07-07)

## Context

With internal UUIDs (ADR 0004), users need a human identifier for commands (`/award trophy:<...>`). The natural one is the trophy **name**. The legacy bot never enforced name uniqueness and resolved names to "first match", which is ambiguous. Verified against production data (2026-07-07): **176 of 2,493 guilds (7%) contain exact duplicate trophy names** (198 guilds if compared case/whitespace-insensitively); e.g. one guild has three trophies named "Do You Smell Barbecue?". There is no way to tell duplicates apart in a command (`trophy:5` vs `trophy:5`).

## Decision

- `UNIQUE(guild_id, name)` constraint on `trophies`. Names may repeat freely across different guilds.
- Commands resolve trophies by name within the guild, with slash-command autocomplete (the autocomplete choice displays the name and can carry the UUID as value).
- **Legacy deduplication rule** (applied by the importer): within a guild, trophies sharing the same name each get the legacy numeric ID appended with a space — `"test"` (id 3) and `"test"` (id 7) become `"test 3"` and `"test 7"`. Same-ID duplicates cannot exist (JSON object keys are unique per guild). If appending exceeds the 32-char name limit, the base name is truncated to fit. If the result still collides with another existing name in the guild, the importer keeps appending disambiguators until unique. Every rename is recorded in the import report.
- Uniqueness is **case- and punctuation-insensitive** (application-level check plus a lower()/normalized index). Validated legacy behavior supports this: `getTrophy()` (globals.js:121-147) normalized names to lowercase stripping all non-word characters and did substring matching, so differently-cased names already collided in resolution. Dedupe grouping at import therefore uses the normalized name (198 affected guilds, not just the 176 with exact duplicates).

## Consequences

- Ambiguity disappears permanently; `/create` and `/edit` must validate name availability.
- Roughly 200-400 historical trophies get renamed at import; guild admins may notice — the import report per guild makes this auditable.
