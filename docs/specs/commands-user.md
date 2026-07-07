# User-Facing Commands — Validated Specification

Validated against the JavaScript source code (the ONLY source of truth) on 2026-07-07.
Files audited in full: `commands/users/leaderboard.js`, `commands/users/show.js`,
`commands/users/trophies.js`, `commands/users/trophystats.js`, plus the shared helpers in
`globals.js` and the dispatcher `events/command.js`. Production data shapes were verified
against `guilds_db.json` (2,493 guilds / 10,853 trophies / 60,554 award entries).

**Shared dispatcher behavior (applies to all commands below):**
- Every command is publicly deferred with `interaction.deferReply()` (`events/command.js:14`) — no user command here is ephemeral.
- `getServer()` initializes the guild's DB structure before any command runs (`events/command.js:21`, `globals.js:405-447`), so guild data paths always exist after first interaction.
- None of the three user command modules define a `permissions` array or `cooldown` field, so the `imsafe` gate (`events/command.js:39-44`) and cooldowns do NOT apply to them.
- BUG (framework-wide): the admin check builds `permissionsFor(member).toArray()` and looks for `'ADMINISTRATOR'` (`events/command.js:37`). discord.js v14 returns PascalCase flag names (`'Administrator'`), so this check is **always false** in v14. It also affects `/trophies guild` (see below).

---

## /leaderboard

### Purpose
Shows the server's score ranking, sorted by each user's accumulated trophy value.

### Definition
| Parameter | Type | Required | Default |
|---|---|---|---|
| `page` | Integer | No | 1 (`leaderboard.js:17`, via `|| 1`, so `0` also becomes 1) |

Permissions: none. Cooldown: none. Reply: public embed.

### Current behavior (validated)
1. Reads all users of the guild: `data.${guild}.users` or `{}` (`leaderboard.js:18`).
2. Calls `attemptFetchIfCacheCleared(keys, guild)` (`leaderboard.js:24`, `globals.js:283-288`): fetches ALL guild members from Discord only if the number of user keys in the DB is greater than the current member-cache size. Otherwise it trusts the cache.
3. Filters users (`leaderboard.js:27`): a user is listed only if
   - `users[key].trophyValue` is **truthy** (so score 0 or missing is excluded; negative scores are included), AND
   - `isInServer(guild, key)` — a pure cache lookup (`globals.js:90-92`) — OR the `hide_quit_users` setting equals 1 ("Show Quit Users"; default is 0 = hide, `globals.js:74-80`).
4. `total` is the sum of **only the visible users'** scores (`leaderboard.js:29`) and is displayed as "Total server score".
5. Sorts descending by stored `trophyValue` (`leaderboard.js:33`).
6. Paginates with `getPage(keys, 10, page)` — 10 users per page; `getPage` clamps the requested page into `[1, last]` and reports `{list, page, last}` (`globals.js:592-600`).
7. Rank numbering starts at `i = ((page - 1) * 10) + 1` (`leaderboard.js:38`) using the **raw** `page` option, not the clamped `pages.page` — see BUG below.
8. Each row is `${getMedal(i)} **${i}.-** ${parse} ➤ **${value}** :medal:` (`leaderboard.js:43`). `getMedal` returns 🥇/🥈/🥉 for i = 1/2/3, else `:medal:` (`globals.js:111-119`).
9. Name rendering uses the `leaderboard_format` setting via `parseFormat` (`globals.js:149-167`):
   - 0 (Mention, default): `<@id>` without any fetch.
   - 1 (Username): `guild.members.fetch(id)` then `user.user.username`.
   - 2 (Nickname): fetch, then `nickname ?? username`.
   - 3 (Username and Tag): fetch, then `user.user.tag`.
10. Footer: `Page {pages.page} of {pages.last}` (uses the clamped values, unlike the rank numbers).

Note: `updatePanel` (`globals.js:602-656`) contains a copy-pasted duplicate of this exact logic (always page 1, no footer) for the persistent leaderboard panel.

### Data operations
- Read `data.${guild}.users`
- Read `data.${guild}.settings.hide_quit_users` (via `getSetting`, `globals.js:95-103`)
- Read `data.${guild}.settings.leaderboard_format`

### Edge cases, quirks and bugs
- **BUG — rank numbering ignores page clamping**: request `page: 999` on a 2-page board and `getPage` shows page 2's users, but ranks start at 9981 and the footer says "Page 2 of 2". Request `page: -3` (truthy, so `|| 1` doesn't apply) and page 1's users are shown with ranks starting at −29 and no medals (`leaderboard.js:38` vs `globals.js:594`).
- **BUG — formats 1/2/3 crash on departed users**: `parseFormat` calls `guild.members.fetch(id)` with no try/catch (`globals.js:153,157,161`). With "Show Quit Users" enabled (or a stale cache), a departed user makes the fetch reject, the whole command throws, and the user sees the generic error embed. The `pre = "Unknown User"` fallback (`globals.js:149`) is unreachable for missing members because fetch throws instead of returning null.
- **QUIRK — zero-score users are invisible**: the truthiness filter (`leaderboard.js:27`) hides anyone with score 0, even active members who earned and then lost points. Negative totals ARE shown (negative numbers are truthy).
- **QUIRK — "Total server score" is not the server total**: it is the sum of currently *visible* rows only, so it changes when members leave (with hide_quit_users) and never counts zero-score users.
- **QUIRK — quit detection is cache-dependent**: `isInServer` only checks the member cache; `attemptFetchIfCacheCleared`'s heuristic (`keys.length > cacheUsers`) can skip the fetch when the cache is partially populated but larger than the DB user count, silently hiding present members or showing absent ones.

### Discrepancies with prior docs
- DISCORD_COMMANDS_DOCUMENTATION.md claims "Medal emojis for top positions" / CLAUDE.md claims "validated page ranges" — rank/medal computation is NOT validated against the clamped page (bug above).
- Prior docs describe `attemptFetchIfCacheCleared` as "Refresh user cache" — it is a conditional heuristic, not a refresh.
- Prior docs say the description shows "Total server score" — it is actually the visible-rows-only sum.
- No prior doc mentions that zero-score users are excluded or that non-mention formats crash on departed users.

### Rust target
- Compute scores as `SUM(trophies.value)` via JOIN over `user_trophies` (no stored `trophyValue`); one SQL query with `ORDER BY score DESC LIMIT 10 OFFSET ...`.
- Fix rank numbering: derive ranks from the clamped page. Keep 10/page and 🥇🥈🥉 for global ranks 1-3.
- Decide explicitly whether zero-score users appear (recommended: include users with at least one award, even at 0, and document it as a deliberate change).
- `hide_quit_users`: resolve membership via Serenity's cache with HTTP fallback per page (only 10 lookups), and NEVER let a failed member fetch abort the command — fall back to mention/`Unknown User`.
- Total server score should be a real aggregate, independent of display filters (or clearly labeled).
- The panel renderer must share this code path (no duplication as in `globals.js:602-656`).

---

## /show

### Purpose
Publicly displays a trophy: emoji, name, description, image, value, signature, dedication.

### Definition
| Parameter | Type | Required | Default |
|---|---|---|---|
| `trophy` | String | Yes | — (name or numeric ID) |

Permissions: none. Cooldown: none. Reply: public embed, possibly with a file attachment.

### Current behavior (validated)
1. Resolves the input with `getTrophy` (`show.js:19`, `globals.js:121-147`):
   - If `parseInt(input)` is a number AND that key exists under `data.${guild}.trophies`, that ID wins immediately (`globals.js:122-129`).
   - Otherwise fuzzy name match: both input and each trophy name are normalized by `parseName` — lowercased with all non-word characters stripped (`globals.js:237-240`) — and a trophy matches if its normalized name **contains** the normalized input as a substring (`checkName`, `globals.js:242-244`). The first match in object-key iteration order wins; since keys are integer-like strings, that means **lowest trophy ID wins**.
   - Returns `null` if nothing matches → error embed "Could not find a trophy..." (`show.js:21-28`).
2. Reads the trophy object (`show.js:30`) and picks the image: `object.image || <default Discord CDN trophy PNG>` (`show.js:33`).
3. Image rendering (`show.js:43,62`): if the stored value starts with `https://` it is used directly as the embed image URL and no file is attached; otherwise it is treated as a local filename — embed image `attachment://<image>` with file `./images/<image>`.
4. Always adds a "Value" field: `​{value} :medal:` (inline), even for value 0 or negative (`show.js:45`).
5. If `signed`, adds "Signed by ​<@creator>" (`show.js:50-52`).
6. Dedication (`show.js:54-58`): if `dedication.name` is set, formats with `getDedication(guild, dedication, config)` (`globals.js:33-49`) using `dedication_display` (default 2):
   - No `dedication.user` id → the stored plain-text name.
   - config 0 ("Always Mention"): `<@id>` without any fetch.
   - config 1 ("Always Name"): fetches the member (errors swallowed, `globals.js:43-45`); if absent → stored name; if present → `user?.username ?? name` — see BUG below.
   - config 2 ("Mention Only in Server", default): fetches the member; present → `<@id>`, absent → stored name.
7. Embed URL is a hardcoded YouTube link (`show.js:40`), footer shows `Trophy ID: {id}` (`show.js:46-48`).

### Data operations
- Read `data.${guild}.trophies` and `data.${guild}.trophies.${id}` (resolution + display)
- `client.db.guilds.has(...)` existence check inside `getTrophy`
- Read `data.${guild}.settings.dedication_display`

### Edge cases, quirks and bugs
- **QUIRK — numeric IDs shadow names**: `parseInt("12th Anniversary")` is NaN (safe), but `parseInt("12 Wins")` is 12 — if trophy ID 12 exists, the user gets trophy 12 instead of the trophy named "12 Wins" (`globals.js:122`).
- **QUIRK — substring matching**: searching "gold" matches "Golden Medal" and any other name containing "gold"; the lowest-ID match is returned with no ambiguity warning.
- **BUG — "Always Name" never shows the live username**: in the config 1 branch, `user` is a `GuildMember`, which has no `.username` property in discord.js v14, so `user?.username` is always `undefined` and it always falls back to the dedication name stored at creation time (`globals.js:47`).
- **BUG risk — missing local image file crashes the command**: if the stored filename no longer exists in `./images/`, `editReply` with the files array throws and the user gets the generic error embed (`show.js:62`). A production incident of this kind is visible in the commented `fixShit()` history (`globals.js:658-683`).
- **QUIRK — stored full-URL images (verified in production)**: 195 of 10,853 trophies store a full `https://cdn.discordapp.com/...` URL in `image` instead of a local filename. Discord CDN attachment URLs now expire, so those embeds render broken images. 2,693 store local filenames; 7,965 store `null` (default image used).
- **QUIRK — dedication object assumed present**: `dedication.name` (`show.js:55`) would throw if a trophy lacked the `dedication` object; verified that all 10,853 production trophies have it, but the importer must not produce rows without it.

### Discrepancies with prior docs
- DISCORD_COMMANDS_DOCUMENTATION.md's dedication table says config 1 returns the "username string" — in reality it always returns the stored dedication name (v14 `GuildMember.username` bug).
- Prior docs describe trophy resolution as "name or ID" but never mention the substring/normalization matching, the lowest-ID tie-break, or the `parseInt` prefix quirk.
- Prior docs do not mention that `image` may hold a full CDN URL in real data (they describe only filename-or-null).

### Rust target
- Trophies are identified by NAME only (unique per guild) with slash-command autocomplete; drop numeric-ID resolution, substring fuzzing, and the `Trophy ID` footer (internal UUIDv7 IDs are never user-facing).
- Keep: default trophy image, value field always shown, "Signed by" mention, the three dedication modes — but implement mode 1 correctly (live username via member/user fetch, fallback to stored dedication text).
- Image serving must not crash on a missing file: fall back to the default image and log.
- Importer note: for the 195 trophies whose `image` is a Discord CDN URL, attempt download at migration time; on failure store NULL (default image) — do not carry expiring URLs into the new schema.

---

## /trophies

Parent command "See a list of trophies." with two subcommands (`trophies.js:5-18`). No permissions, no cooldown, public replies.

### /trophies user

#### Purpose
Lists a user's earned trophies, aggregated by type with counts, plus their total score.

#### Definition
| Parameter | Type | Required | Default |
|---|---|---|---|
| `user` | User | No | command invoker (`trophies.js:31`) |
| `page` | Integer | No | 1, floored and clamped to ≥1 (`trophies.js:32`) |

#### Current behavior (validated)
1. Reads stored score `data.${guild}.users.${user}.trophyValue` (denormalized, `trophies.js:34`) and the award array `...users.${user}.trophies` (`trophies.js:36`), both defaulting to 0 / [].
2. For each award ID, loads the trophy object; **silently skips IDs whose trophy no longer exists** (`trophies.js:40-48`). Existing ones are aggregated into `{id, value, count}`.
3. Resolves the display name via `parseUser(client, user, interaction.user.username, guild, true)` (`trophies.js:50`, `globals.js:507-544`): a snowflake ID goes through `client.users.fetch` (global, works for users who left the guild); on failure the fallback is the **invoker's username string** — see BUG.
4. Sorts aggregated entries by trophy value descending (`trophies.js:53-57`).
5. Row format (`trophies.js:62-73`): `{emoji} {name} **+{value}** _x{count}_`; positive values get a `+` prefix, value 0 renders no value at all, negatives render as-is.
6. Paginates 10 per page (`trophies.js:75`), title `{username}'s Trophies`, description `Total score: **{score}** :medal:`, footer `Page x of y`.

#### Data operations
- Read `data.${guild}.users.${user}.trophyValue`
- Read `data.${guild}.users.${user}.trophies`
- Read `data.${guild}.trophies.${id}` for each award entry

#### Edge cases, quirks and bugs
- **QUIRK — orphaned award IDs (importer-critical)**: awards referencing deleted trophies are hidden from the list but remain in the array, and the stored `trophyValue` is whatever `/delete`'s `cleanseTrophies` left behind (`globals.js:252-265`). Displayed "Total score" can therefore disagree with the sum of the visible rows if data ever got out of sync. Verified: all 60,554 production award entries are **strings**; the importer must drop entries whose ID has no matching trophy and recompute scores.
- **BUG — "undefined's Trophies"**: if `client.users.fetch` fails (deleted account), `parseUser` returns the fallback — a plain string — and `userObject.username` is `undefined`, so the title renders as `undefined's Trophies` (`trophies.js:50-51`).
- **QUIRK — fallback is the invoker's name**: even when it "works", the `notfound` fallback passed is the *command invoker's* username, not the target user's.
- **QUIRK**: querying a user with no data shows "Total score: **0**" and "No trophies yet." (defaults at `trophies.js:34,36,81`).

#### Discrepancies with prior docs
- Prior docs say duplicates render "e.g. _x3_" — correct, but they omit that a single award also renders `_x1_`, that positive values get a `+` prefix, and that zero-value trophies show no value.
- No prior doc mentions the silent skipping of orphaned trophy IDs or the `undefined`-title bug.

#### Rust target
- Score and per-trophy counts come from SQL (`SUM`/`COUNT` grouped join on `user_trophies × trophies`); orphans cannot exist thanks to FK + CASCADE, so list and total always agree.
- Keep: aggregation with `xN` counts, value-descending order, 10/page, `+` prefix and zero-value omission (cosmetic parity).
- Resolve the target's display name from the interaction's resolved user data (Poise gives the `User` object directly) — no fetch-failure title bug possible.

### /trophies guild

#### Purpose
Lists all trophies defined in the server, optionally hiding never-awarded ones.

#### Definition
| Parameter | Type | Required | Default |
|---|---|---|---|
| `page` | Integer | No | 1, floored and clamped to ≥1 (`trophies.js:94`) |

#### Current behavior (validated)
1. Reads `data.${guild}.trophies`, drops the `current` counter key (`trophies.js:95-96`), sorts IDs by trophy value descending (`trophies.js:98-101`).
2. Computes whether to hide unused trophies (`trophies.js:103-110`): hidden only when the viewer is neither a `manage_trophies` permission-role holder nor an administrator, AND `hide_unused_trophies == 0` ("Hide"; default is 1 = Show, `globals.js:67-73`). Both viewer checks are broken — see BUGS.
3. When hiding, a trophy survives only if at least one user's award array `includes` its ID — a full scan of every user's array per trophy (`trophies.js:114-120`).
4. Row format identical to `/trophies user` minus the count: `{emoji} {name} **+{value}**` (`trophies.js:123-133`).
5. Description shows `Total trophies created: **{N}**` using the count **before** the unused filter (`trophies.js:140`), then paginates the filtered list 10 per page (`trophies.js:135`).

#### Data operations
- Read `data.${guild}.trophies`
- Read `data.undefined.permissions.manage_trophies` (sic — see BUG)
- Read `data.${guild}.settings.hide_unused_trophies`
- Read `data.${guild}.users`

#### Edge cases, quirks and bugs
- **BUG — permission roles are read from a garbage path**: `guild` here is the guild ID string, so `` `data.${guild.id}.permissions.manage_trophies` `` resolves to `data.undefined.permissions.manage_trophies` (`trophies.js:103`), which is always undefined → `permroles` is always `[]` and `isPerm` is always false.
- **BUG — admin detection always false in v14**: `toArray().includes('ADMINISTRATOR')` (`trophies.js:106`) never matches v14's `'Administrator'`. Combined with the path bug, `hideUnusedTrophies` reduces to `setting == 0` for EVERY viewer, administrators included.
- **QUIRK — total vs list mismatch**: "Total trophies created" counts all trophies even when the unused filter hides some, so the number can exceed the listed items across all pages.
- **QUIRK — O(users × trophies) scan** of every award array for the unused filter; slow for large guilds.
- Sorting reads `object[b].value` for every comparison; a trophy object that is somehow not an object would throw (not observed in production data).

#### Discrepancies with prior docs
- DISCORD_COMMANDS_DOCUMENTATION.md claims "Checks if user has manage_trophies permission / is administrator" and "Different views based on user permissions" — both checks are dead code (bugs above); in practice the setting alone decides, for everyone.
- CLAUDE.md's `hide_unused_trophies` description ("affects non-managers viewing guild trophies") describes the intent, not the actual behavior.

#### Rust target
- One SQL query: trophies of guild ordered by value DESC; "unused" = `NOT EXISTS (SELECT 1 FROM user_trophies ...)` — no full scans.
- Implement the intended exemption correctly: viewers with Manage Guild (or Administrator) always see unused trophies; the deprecated custom `manage_trophies` roles are NOT carried over (deprecated in JS, and non-functional here anyway).
- Decide whether "Total trophies created" counts hidden trophies (recommended: show the visible count, or both).
- Keep 10/page and the row format.

---

## /trophystats — NOT TO IMPLEMENT

Confirmed: `commands/users/trophystats.js` is a completely empty file (0 bytes). It is
loaded by `fetchModules` like any other `.js` file, but since `require` of an empty module
yields `{}` with no `data.name`, it is skipped and never registered as a command
(`globals.js:308-313`). It has never been functional and must not be implemented in the
Rust rewrite. (Prior docs agree on this point.)
