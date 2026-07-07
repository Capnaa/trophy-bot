# Trophy Management Commands — Validated Behavior Spec

This document describes the REAL behavior of the trophy lifecycle commands, validated
directly against the JavaScript source code (discord.js v14 + quick.db 7.1.3) on 2026-07-07.
The only source of truth is the code in `commands/manage/*.js`, `globals.js` and
`events/command.js`. Prior AI-generated docs (DISCORD_COMMANDS_DOCUMENTATION.md,
CLAUDE.md) contain errors; discrepancies are called out per command.

## Shared behavior (events/command.js, globals.js)

All slash commands go through `events/command.js`:

- Every command interaction is deferred publicly (`command.js:14`) — replies are **never ephemeral**, including error replies.
- Guild data is initialized on first command via `getServer` (`globals.js:405-447`), which creates `trophies: {current: 0}`, `users: {}`, `rewards: []`, and notably `imsafe: true` for **new** guilds (`globals.js:437`).
- The imsafe gate applies only to commands that export a `permissions` property (`command.js:39-44`): if `data.${guild}.imsafe` is falsy, `imsafeWarning` (`globals.js:21-31`) is shown instead of running the command. Bot dev (hardcoded ID `353998390734094346`, `globals.js:388-390`) bypasses the gate.
- The legacy `permissions` arrays (`manage_trophies`, `manage_users`) are **never actually checked** against roles — they only serve as the marker that triggers the imsafe gate.
- Command counters `data.commands.${name}` and `data.commands.total` are incremented before the command runs (`command.js:46-49`), even if it later errors.

### getTrophy — trophy resolution by name or ID (globals.js:121-147)

This is the resolver used by award, revoke, delete, edit, details (and show). Exact semantics:

1. **ID branch** (`globals.js:122-129`): if `parseInt(input)` is not NaN (i.e. the input *starts* with digits, optionally signed), the raw input string is interpolated into the quick.db path `data.${guild}.trophies.${input}`. If that path exists, the **raw input string** is returned as the ID.
   - ID match takes absolute precedence: a trophy literally *named* "5" can never be found by name if a trophy with ID 5 exists.
   - BUG (path traversal): the raw input is used as a quick.db dot path. Input `1.name` parses as number 1 and `has('...trophies.1.name')` is true if trophy 1 exists — getTrophy returns `"1.name"` as a "valid" trophy ID. Callers then `get('...trophies.1.name')` which returns the *name string* as the trophy object; in `/award` this pushes the bogus ID `"1.name"` into the user's trophy array and adds `NaN` to their score (`object.value` is undefined). Data-corruption vector.
2. **Name branch** (`globals.js:131-144`): the input is normalized by `parseName` (`globals.js:237-240`): lowercased, then **all non-`[A-Za-z0-9_]` characters stripped** (spaces, punctuation, emoji, accented letters — note `\W` removes `é`, `ñ`, etc.). It then iterates all trophy entries (skipping the `current` counter key) and returns the **first** trophy whose normalized stored name **contains the normalized input as a substring** (`checkName`, `globals.js:242-244`: `storedName.includes(input)`).
   - Matching is therefore: case-insensitive, punctuation/space-insensitive, **partial substring** (input `gold` matches trophy "Golden Medal").
   - On multiple matches, iteration order of a JS object with integer-like keys applies: **lowest numeric ID wins**.
   - QUIRK: input that normalizes to the empty string (emoji-only or punctuation-only input, e.g. `🏆`) matches **every** trophy (`"x".includes("") === true`), so the trophy with the lowest ID is returned.
3. Returns `null` if nothing matched.

### doRewardRoles (globals.js:169-235)

Called after award/revoke/clear, always wrapped in an empty `catch` by callers.

- BUG (high confidence, code-level): `me.permissions.has('MANAGE_ROLES')` (`globals.js:178`) uses the discord.js **v13** flag name. In v14 (`package.json`: `discord.js ^14.6.0`) permission flags are PascalCase (`ManageRoles`); the string `'MANAGE_ROLES'` makes `BitField.resolve` throw, the surrounding try/catch returns (`globals.js:180-182`), and **role rewards are silently never applied**. (`guild.me` at `globals.js:171` is also undefined in v14, but there is a fetch fallback.)
- The score used is the **stored** `user.trophyValue` (`globals.js:199`), not a recomputed sum — any desync propagates to role decisions.
- Rewards are stored sorted by requirement descending (sorted at insert time in rewards.js); with `stack_roles = 0` all qualifying roles are awarded, otherwise only the highest, with lower/non-qualifying roles queued for removal.

### Other shared helpers

- `getTrophyCount` (`globals.js:246-250`): `Object.getOwnPropertyNames(trophies).length - 1` (subtracting the `current` key). QUIRK: returns -1 when the guild has no trophies object (defaults to `{}`).
- `cleanseTrophies` (`globals.js:252-265`): for every user, removes **all** occurrences of a trophy ID (strict `===` matching via `includes`/`indexOf`) and subtracts the trophy's value per occurrence, then rewrites the whole `users` object. Does not call doRewardRoles.
- `parseUser` (`globals.js:507-544`): resolves mention (`<@id>`), raw snowflake (via `client.users.fetch`), or falls back to `guild.members.search({query, limit: 1})` — a **prefix search** on member names. QUIRK: a plain-text dedication like "Mom" silently resolves to any member whose name starts with "Mom".
- `downloadImage` (`globals.js:500-504`): fetches URL and writes bytes to disk. No content-type or size verification of the downloaded body.

---

## /create

**Purpose:** Create a new trophy in the guild (name, description, emoji, value, dedication, image, private details, signature).

### Definition

| Param | Type | Required | Default (create.js:39-46) |
|---|---|---|---|
| name | String | yes | `'New Trophy'` fallback (dead — option is required) |
| description | String | no | `'No description provided'` |
| emoji | String | no | `':trophy:'` |
| value | **Number** (float allowed) | no | `10` |
| dedication | String | no | `null` |
| signed | Boolean | no | `false` |
| image | Attachment | no | `null` |
| details | String | no | `'No details provided.'` |

Discord default permission: Manage Guild (`"32"`, create.js:8). Legacy marker: `manage_trophies` → imsafe gate applies. No cooldown.

### Current behavior (validated)

1. Reject if `getTrophyCount >= 150` (create.js:24-37).
2. Validate, in order: name ≤32 (create.js:52), details ≤300 (create.js:62), description ≤128 (create.js:72), emoji ≤64 (create.js:82), value within ±999999 (create.js:92), dedication ≤32 (create.js:102). Each failure returns an error embed.
3. Dedication parsing (create.js:119-143): if provided, `parseUser` tries mention/ID/member-prefix-search; on success dedication is `{user: id, name: username}`, otherwise `{user: null, name: <raw text>}`. If no dedication was given, dedication is stored as an **empty object `{}`** (not `{user: null, name: null}`).
4. Increment `trophies.current` and use it as the new ID (create.js:146-147).
5. Store the full trophy object at `data.${guild}.trophies.${id}` (create.js:151-162). Image filename `${guild}_${id}.${extension}` is stored if an attachment was passed — **before the image is validated**.
6. Only now validate the image (create.js:164-184): extension (from the attachment *filename*, `split('.').pop()`) must be png/jpg/jpeg/gif; size must be ≤ 1,000,000 bytes (decimal MB). On failure it **returns an error, leaving the already-stored trophy in the DB** with a dangling image filename, the ID counter consumed, and the global counter not incremented.
7. If valid, `downloadImage` saves to `./images/${guild}_${id}.${extension}` (create.js:186).
8. Increment global `data.trophies` via get-then-set (create.js:200-201; not atomic).
9. Success embed shows emoji/name/description, value, optional "Signed by"/"Dedicated to" fields, footer `Trophy ID: ${id}` (create.js:112-198).

### Validation rules & limits

- 150 trophies/guild (`>= 150` check, so 150 is the hard cap).
- name ≤32, description ≤128, emoji ≤64, dedication ≤32, details ≤300 chars; value in [-999999, 999999].
- Value is a **Number option: fractional values (e.g. 10.5) are accepted and stored** — legacy data may contain floats.
- Duplicate trophy names are **not** prevented.
- Image: extension ∈ {png,jpg,jpeg,gif} (by filename), size ≤ 1,000,000 bytes.

### Data operations

- Read: `data.${guild}.trophies` (count), `data.${guild}.trophies.current`, `data.trophies`.
- Write: `data.${guild}.trophies.current` (+1), `data.${guild}.trophies.${id}` (set), `data.trophies` (set).

### Edge cases, quirks and bugs

- BUG: trophy is persisted before image validation; a rejected image leaves a half-created trophy (with a phantom image filename) and shows an error to the user (create.js:151-184).
- BUG/QUIRK: value accepts floats despite docs/target schema assuming integers.
- QUIRK: no duplicate-name check — multiple trophies with identical names allowed (getTrophy will always resolve the lowest ID).
- QUIRK: empty dedication stored as `{}`, not `{user: null, name: null}` — consumers must handle both shapes.
- QUIRK: global `data.trophies` counter uses non-atomic read-then-write.
- QUIRK: image extension comes from the filename, not content-type; no verification of downloaded bytes.

### Discrepancies with prior docs

- DISCORD_COMMANDS_DOCUMENTATION.md presents image validation as a precondition; in reality it runs **after** the DB write (the half-created-trophy bug is undocumented).
- CLAUDE.md's trophy object shows dedication always as `{user, name}`; empty dedication is actually `{}`.
- Neither doc mentions that `value` is a float-capable Number option, nor that duplicate names are allowed.

### Rust target

- Keep: field limits (32/128/64/32/300, ±999999), image constraints (png/jpg/jpeg/gif, 1 MB), dedication parsing (mention/ID/text — but drop the accidental member prefix-search or make it explicit).
- Change: validate **everything (including image) before** any DB write, inside one transaction. Trophy PK is UUIDv7, never shown to users; **name becomes UNIQUE per guild** — reject duplicates at create (migration renames legacy dupes to `"name legacyId"`). Value becomes an integer column. Drop the 150 cap (or raise via config). Store the image filename string as-is; keep files on disk. Store dedication as nullable `dedication_user_id` + nullable `dedication_text`.

---

## /edit

**Purpose:** Edit an existing trophy's name, description, emoji, dedication, details or image. Value and signed flag cannot be edited.

### Definition

| Param | Type | Required |
|---|---|---|
| trophy | String | yes |
| name, description, emoji, dedication, details | String | no |
| image | Attachment | no |

Discord default permission: Manage Guild (edit.js:8). Legacy marker `manage_trophies` → imsafe gate. No cooldown. **There is no `value` option** — it is commented out (edit.js:47,139).

### Current behavior (validated)

1. Resolve trophy via `getTrophy` (edit.js:26); error if not found.
2. Merge: each provided option overrides the current field using `||` (edit.js:44-50); `value`, `signed`, `creator`, `created` always preserved (edit.js:39-42).
3. Validate merged values: name ≤32, details ≤300, description ≤128, emoji ≤64, dedication ≤32 (edit.js:54-101).
4. Dedication: `"-"` resets to `{user: null, name: null}` (edit.js:107-111); otherwise same parseUser logic as create (edit.js:114-131). If not provided, existing dedication kept.
5. Change tracking (edit.js:135-152): a diff list is built; if empty, error "No changes were made".
6. Image handling (edit.js:48,154-192): `image` is the **new attachment's URL** or the current stored filename. If it differs from the stored value, extension is derived from `url.split('.').pop()` (which includes the CDN query string, hence the `startsWith(ext)` check at edit.js:165), validated, downloaded to `./images/${guild}_${id}.${extension}` — a filename **containing the query string**.
7. The whole trophy object is rewritten at `data.${guild}.trophies.${id}` (edit.js:194-205) — with `image: image`, i.e. the **full Discord CDN URL**, not a local filename.

### Validation rules & limits

Same text limits as create. Image: extension prefix ∈ {png,jpg,jpeg,gif}; the 1 MB size check is broken (see bugs).

### Data operations

- Read: `data.${guild}.trophies` (via getTrophy), `data.${guild}.trophies.${id}`.
- Write: `data.${guild}.trophies.${id}` (full object rewrite).

### Edge cases, quirks and bugs

- BUG: on image change, the DB stores the ephemeral Discord CDN **URL** instead of the local filename (edit.js:201), inconsistent with create. show.js renders URLs directly (`show.js:43,62`), so it works until the signed CDN URL expires; the downloaded local file (with query-string junk in its name) is never referenced.
- BUG: size check `image.size > 1000000` (edit.js:181) — `image` is a URL string here; `.size` is undefined, so the 1 MB limit is **never enforced** on edit.
- BUG/QUIRK: editing the dedication to the same value still counts as a change (edit.js:140 checks only `if (dedic)`); removing a dedication with `"-"` prints the change as `... > null`.
- QUIRK: `||` merging means a field can never be set to an empty string (Discord prevents empty options anyway).
- QUIRK: leftover `console.log(extension)` debug output (edit.js:157).
- QUIRK: value and signed are silently immutable; there is no way to change a trophy's value after creation.

### Discrepancies with prior docs

- DISCORD_COMMANDS_DOCUMENTATION.md claims edit validation is "Same as /create command for all fields" — false: value can't be edited, the 1 MB image check is inoperative, and the extension check is `startsWith` on a query-string-polluted token.
- Prior docs don't mention the image-stored-as-URL inconsistency nor the change-tracking dedication quirk.

### Rust target

- Keep: `"-"` sentinel to clear dedication (or replace with an explicit boolean option); change-diff in the success reply; name/emoji/desc/details limits.
- Change: allow editing `value` (the omission looks accidental, decide product-side); enforce image size and type properly using attachment metadata; always store the **local filename** consistently; validate before writing; renaming must respect the per-guild unique name constraint. Trophy resolved by name (unique), not numeric ID.

---

## /delete

**Purpose:** Delete a trophy from the guild, remove it from every user, and delete its image file.

### Definition

Param: `trophy` (String, required). Discord default permission: Manage Guild (delete.js:9). Legacy marker `manage_trophies` → imsafe gate. No cooldown. **No confirmation step.**

### Current behavior (validated)

1. Resolve via `getTrophy` (delete.js:22); error if not found.
2. Read the trophy object for value/name/image/emoji (delete.js:33-37), then delete `data.${guild}.trophies.${id}` (delete.js:39).
3. Decrement global `data.trophies` with a floor of 0, via get-then-set (delete.js:41-42).
4. `cleanseTrophies` (delete.js:45, globals.js:252-265): removes every occurrence of the ID from every user's `trophies` array and subtracts `value` per occurrence from their `trophyValue`, then rewrites the entire `users` object.
5. Delete `./images/${image}` with errors ignored (delete.js:51) — attempts `./images/null` when the trophy had no image.
6. Success embed. **Role rewards are NOT recomputed** for affected users.

### Validation rules & limits

None beyond trophy existence.

### Data operations

- Read: `data.${guild}.trophies` (getTrophy), `data.${guild}.trophies.${id}`, `data.trophies`, `data.${guild}.users`.
- Write: delete `data.${guild}.trophies.${id}`; set `data.trophies`; set `data.${guild}.users` (bulk rewrite).

### Edge cases, quirks and bugs

- QUIRK: destructive, un-confirmed, and rewrites the whole users blob (large guilds: heavy write).
- QUIRK: no `doRewardRoles` after cleansing — users can keep reward roles their reduced score no longer justifies.
- QUIRK: `cleanseTrophies` uses strict `===` matching; if legacy arrays contain numeric IDs while the resolver returns strings (or vice versa), occurrences are missed, orphaning trophy IDs in user arrays while `trophyValue` stays inflated.
- QUIRK: `fs.unlink('./images/null')` when image is null (harmless, error swallowed).

### Discrepancies with prior docs

- DISCORD_COMMANDS_DOCUMENTATION.md/CLAUDE.md describe delete as clean; they omit that role rewards are not recomputed and the whole-users rewrite.

### Rust target

- Keep: cascade removal of awards, image file cleanup, global counter maintenance (or derive counts by query instead).
- Change: single transaction — soft-delete trophy row; `user_trophies` rows removed (or cascade via FK); score needs no fixup because it is always `SUM(value)` via JOIN; recompute role rewards for affected users (or accept eventual recompute on next award). Add a confirmation button for destructive delete. Trophy identified by unique name.

---

## /award

**Purpose:** Award N copies of a trophy to a user and update their score and reward roles.

### Definition

| Param | Type | Required | Default |
|---|---|---|---|
| trophy | String | yes | — |
| user | User | yes | — |
| count | Integer | no | 1 |

Discord default permission: Manage Guild (award.js:8). Legacy marker `manage_users` → imsafe gate. No cooldown.

### Current behavior (validated)

1. `count = Math.floor(Math.max(count || 1, 1))` (award.js:23) — 0/negative inputs silently become 1.
2. Resolve trophy via `getTrophy`; two identical "could not find" errors for null ID or missing object (award.js:25-45).
3. Reject if `count < 0 || count > 50` (award.js:47) — the `< 0` branch is unreachable after step 1; effective range is 1–50. Error text says "between 0 and 50".
4. Push the resolved ID string into the user's `trophies` array `count` times (award.js:57-65). The target user does **not** need to be a guild member.
5. `add` `object.value * count` to `data.${guild}.users.${user}.trophyValue` and `count` to global `data.trophiesAwarded` (award.js:68-69, atomic quick.db `add`).
6. `doRewardRoles` in an empty try/catch (award.js:71-73) — see shared section: effectively a no-op under discord.js v14 (BUG).
7. Success embed: "Successfully awarded **N** trophies of ... to @user". Who awarded is **not recorded**.

### Validation rules & limits

- count effectively 1–50; value math uses the trophy's (possibly float) value.
- No check that the user is in the server, no self-award restriction.

### Data operations

- Read: `data.${guild}.trophies` (getTrophy), `data.${guild}.trophies.${id}`, `data.${guild}.users.${user}.trophies`.
- Write: `data.${guild}.users.${user}.trophies` (set), `data.${guild}.users.${user}.trophyValue` (add), `data.trophiesAwarded` (add).

### Edge cases, quirks and bugs

- BUG (inherited): the getTrophy path-traversal input (e.g. `1.name`) awards a garbage ID and adds `NaN` to `trophyValue`, poisoning the score permanently.
- BUG (inherited): role rewards never actually applied (doRewardRoles v14 flag bug), and any error is swallowed.
- QUIRK: count 0 or negative → silently coerced to 1 (not an error as docs claim); the range error message ("between 0 and 50") is unreachable for the low bound.
- QUIRK: dead `if (!interaction) return;` guards (award.js:30,41,51,79).
- QUIRK: `awarded_by` is untracked — the legacy data has no record of who awarded.

### Discrepancies with prior docs

- DISCORD_COMMANDS_DOCUMENTATION.md: "Count must be between 1-50 (inclusive)" as a validation — actually sub-1 values are coerced, not rejected.
- CLAUDE.md: "Check role rewards and assign if threshold reached" — in the running v14 code this silently does nothing.
- Neither doc describes getTrophy's substring/lowest-ID matching, which materially affects which trophy gets awarded on ambiguous names.

### Rust target

- Keep: 1–50 count limit (decide: reject vs coerce — recommend reject with clear error), bulk award as N rows.
- Change: insert N rows into `user_trophies` (one row per award, duplicates allowed) in a transaction, recording `awarded_by` and `created_at`; score is never stored — always `SUM(trophies.value)` via JOIN; recompute role rewards after commit (working implementation, correct Serenity permission checks); resolve trophy by unique name (autocomplete recommended); no NaN class of bugs possible with typed columns.

---

## /revoke

**Purpose:** Remove up to N copies of a trophy from a user and subtract the corresponding score.

### Definition

Same options as /award: `trophy` (String, required), `user` (User, required), `count` (Integer, optional, default 1). Discord default permission: Manage Guild (revoke.js:8). Legacy marker `manage_users` → imsafe gate. No cooldown.

### Current behavior (validated)

1. Same count coercion (min 1) and 1–50 effective range as award (revoke.js:23,45-53).
2. Resolve trophy via `getTrophy`; error if not found (revoke.js:25-43).
3. `amount` = occurrences of the resolved ID in the user's array via strict `===` filter (revoke.js:55-56); `all = count >= amount` (revoke.js:58).
4. Subtract `object.value * min(count, amount)` from `trophyValue`; loop `min(count, amount)` times calling `trophies.pop(id)` (revoke.js:60-66).
5. Write back the array, `subtract` the value, `doRewardRoles` (no-op, see shared), success embed saying "removed **all**" when `count >= amount`, otherwise "removed **count**" (revoke.js:68-80).

### Validation rules & limits

- count effectively 1–50 (sub-1 coerced to 1); revocation capped at the number the user actually holds (for the *score* math and loop count).

### Data operations

- Read: `data.${guild}.trophies` (getTrophy), `data.${guild}.trophies.${id}`, `data.${guild}.users.${user}.trophies`.
- Write: `data.${guild}.users.${user}.trophies` (set), `data.${guild}.users.${user}.trophyValue` (subtract).

### Edge cases, quirks and bugs

- **BUG (confirmed, revoke.js:64): `trophies.pop(id)`** — `Array.prototype.pop` takes no arguments; the `id` is ignored and the **last element of the array is removed, whatever trophy it is**. Meanwhile the score subtraction uses the *requested* trophy's value. Net effect: revoking trophy A can delete the user's copies of trophy B while subtracting A's value — desyncing `trophies[]` from `trophyValue` and from reality. Only coincidentally correct when the last array entries happen to be the requested trophy (e.g. right after an award).
- QUIRK: if the user holds 0 copies, `min(count, 0) = 0` → nothing changes, but `all` is true, so the bot replies "Successfully removed **all** trophies ..." — a success message for a no-op.
- QUIRK: strict `===` in the occurrence filter — numeric-vs-string ID mismatches in legacy arrays make `amount` 0 (silent "removed all" no-op).
- QUIRK: same unreachable `< 0` branch and "between 0 and 50" message as award.

### Discrepancies with prior docs

- DISCORD_COMMANDS_DOCUMENTATION.md claims "Handles partial revocation correctly" and lists "Cannot revoke more than user has" as a validation rule — both wrong: partial revocation removes the wrong elements (pop bug) and over-revoking is silently capped with an "all" success message, not rejected.
- The docs do mention `pop()` but not that it ignores its argument — the central bug is undocumented.

### Rust target

- **FIX the pop bug**: delete exactly N `user_trophies` rows matching the requested trophy (e.g. `DELETE ... WHERE id IN (SELECT id FROM user_trophies WHERE guild_id=? AND user_id=? AND trophy_id=? ORDER BY created_at DESC LIMIT N)`), in a transaction.
- Score self-heals since it is always computed by JOIN. Recompute role rewards after commit. Reply should state the real number removed; revoking when the user holds none should be an explicit informative message, not a fake success.

---

## /clear

**Purpose:** Remove all trophies from a user and reset their score to 0.

### Definition

Param: `user` (User, required) — QUIRK: its description string is a copy-paste, "User to award the trophy to" (clear.js:10). Discord default permission: Manage Guild (clear.js:8). Legacy marker `manage_users` → imsafe gate. No cooldown. No confirmation.

### Current behavior (validated)

1. Unconditionally set `data.${guild}.users.${user}.trophies = []` and `trophyValue = 0` (clear.js:21-22) — this **creates** the user record if it did not exist.
2. `doRewardRoles` in empty try/catch (clear.js:24-26) — no-op under v14 (shared BUG), so reward roles are **not** removed despite the score reset.
3. Success embed (clear.js:28-34). No existence check, no report of what was cleared.

### Validation rules & limits

None. Clearing a user with no data "succeeds".

### Data operations

- Write: `data.${guild}.users.${user}.trophies` (set `[]`), `data.${guild}.users.${user}.trophyValue` (set `0`).

### Edge cases, quirks and bugs

- QUIRK: clearing an unknown user creates an empty record for them.
- BUG (inherited): reward roles are not actually stripped (doRewardRoles no-op).
- QUIRK: wrong option description shown in the Discord UI.

### Discrepancies with prior docs

- DISCORD_COMMANDS_DOCUMENTATION.md: "doRewardRoles(): Update user's reward roles (likely remove all)" — in practice nothing happens (v14 bug).

### Rust target

- Change: `DELETE FROM user_trophies WHERE guild_id=? AND user_id=?` in a transaction (no user row needed — score is computed); recompute (remove) reward roles with the fixed implementation; consider a confirmation step and report how many awards were cleared. Fix the option description.

---

## /details

**Purpose:** Show the private `details` text of a trophy (manager-facing counterpart of the public `/show`).

### Definition

Param: `trophy` (String, required). Discord default permission: Manage Guild (details.js:6). **No `permissions` marker** → the imsafe gate does NOT apply to this command (details.js:4-9 exports no `permissions` key), unlike the other management commands. No cooldown.

### Current behavior (validated)

1. Resolve via `getTrophy` (details.js:20); error if not found.
2. Read the trophy and show an embed: title `${emoji} ${name}`, description = `details` (defaulting to "No details provided.", details.js:32), footer `Trophy ID: ${id}`, embed URL hardcoded to `https://www.youtube.com/watch?v=PwP9ebvCBAM` (details.js:36-42).
3. The reply is **public** (deferReply is never ephemeral) — the "private details" are posted in the channel for everyone to read.

### Validation rules & limits

None beyond trophy existence.

### Data operations

- Read: `data.${guild}.trophies` (getTrophy), `data.${guild}.trophies.${id}`.

### Edge cases, quirks and bugs

- QUIRK: "private" details are shown in a public, non-ephemeral message.
- QUIRK: no imsafe gate (inconsistent with the rest of the management set; only Discord's default Manage Guild permission protects it).
- QUIRK: hardcoded YouTube URL on the embed title (easter egg).

### Discrepancies with prior docs

- CLAUDE.md states "'imsafe' mode required for management commands" — /details is a management-permission command that bypasses the imsafe gate.
- Prior docs list unused imports (`getSetting`, `getDedication` are imported at details.js:2 but never used) as functionality; they are dead code.

### Rust target

- Keep: manager-only visibility of the details field; the Trophy-ID footer becomes the trophy **name** (UUIDs are never user-facing).
- Change: make the reply **ephemeral** so private details stay private; resolve by unique name with autocomplete; drop the easter-egg URL or keep deliberately.

---

## Cross-cutting notes for the Rust rewrite

- **Name resolution**: replicate getTrophy's user-facing ergonomics (case/punctuation-insensitive, substring) as *autocomplete* over the unique per-guild name, but command execution should resolve an exact (case-insensitive) unique name — no silent lowest-ID tiebreaks, no empty-string match-all, no ID/path-injection branch (UUIDv7 PKs are internal only).
- **Score**: always `SUM(trophies.value)` via JOIN over `user_trophies`; never store it. This retires the entire class of trophyValue-desync bugs (revoke pop bug, NaN poisoning, cleanse mismatches).
- **Role rewards**: reimplement correctly (the Node version is dead code under v14) and recompute after award/revoke/clear inside/after the transaction.
- **Awards**: one row per award with `awarded_by` and timestamps; migrated legacy awards get synthetic timestamps and `awarded_by` NULL/0.
- **Images**: keep files on disk; DB stores the filename/URL string as-is; fix edit to store filenames consistently and enforce size/type from attachment metadata before download.
