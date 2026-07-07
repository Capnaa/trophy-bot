# Server Administration Commands — Validated Spec

Validated against the actual Node.js source code (discord.js 14.21.0 as resolved by package-lock.json — ^14.6.0 declared in package.json — quick.db 7.1.3) on 2026-07-07.
Source of truth: `commands/manage/export.js`, `panel.js`, `perms.js`, `rewards.js`, `settings.js` and shared code in `globals.js` / `events/command.js`. Prior markdown docs (DISCORD_COMMANDS_DOCUMENTATION.md, CLAUDE.md) are AI-generated and are corrected here where wrong. The superseded documents referenced in the discrepancy sections live in `docs/archive/`.

**Shared dispatch behavior (events/command.js):** every command is publicly deferred with `interaction.deferReply()` (command.js:14) — no command in this scope ever replies ephemerally, regardless of code comments. The `imsafe` gate applies ONLY to commands exporting a `permissions` array (command.js:39-44): among this scope that is `/panel` (`['manage_users']`, panel.js:5) and `/rewards` (`['manage_rewards']`, rewards.js:5). `/export`, `/permissions` and `/settings` have no `permissions` field and are NOT gated. When gated and `data.${guild}.imsafe` is falsy, `imsafeWarning()` (globals.js:21-31) is shown instead of running. New guilds are initialized with `imsafe: true` (globals.js:437), so the gate only affects pre-1.4 guilds. A `client.cooldowns` collection is created (events/ready.js:28) but no cooldown enforcement exists in the dispatcher; declared `cooldown` fields are dead.

---

## /export

**Purpose:** Dump the guild's entire raw quick.db subtree as a JSON file attachment.

**Definition:**
- Parameters: none
- Discord default permissions: `"8"` = Administrator (export.js:6)
- Cooldown: none
- No `permissions` array → not gated by imsafe

**Current behavior (validated):**
1. Reads the whole guild blob `data.${guild}` (export.js:14).
2. If falsy, replies "No data found for this guild to export." (export.js:15-19) — unreachable dead code in practice: the dispatcher's `getServer()` (events/command.js:21) creates the guild subtree before any command runs, so `data.${guild}` is always truthy by export.js:14. Parity tests should not try to exercise it.
3. Serializes with `JSON.stringify(filedata, null, 2)` and writes `export-${guild}.json` to the process working directory (export.js:21-28).
4. Sends the file as an attachment via `editReply` (export.js:37-40), then deletes the local file (export.js:42).
5. On any error, logs to console and replies "There was an error exporting the data." (export.js:44-48).

**Validation rules & limits:** none beyond the Administrator default permission. Not owner-only.

**Data operations:**
- Read: `data.${guild}` (entire guild subtree: trophies, users, settings, rewards, permissions, panel, imsafe, restapi, language).

**Edge cases, quirks and bugs:**
- QUIRK: The reply is PUBLIC. A comment says "Keep the reply visible only to the user" (export.js:39) but no ephemeral flag is set and the deferral is public — the full data dump (including private trophy `details`) is posted in the channel for everyone.
- QUIRK: Debug leftover `console.log(file)` (export.js:34).
- QUIRK: Filename is unique per guild but not per invocation; concurrent exports of the same guild race on the same temp file (export.js:21).
- QUIRK: The export is the raw legacy structure (includes internal keys like `trophies.current`, `restapi`, deprecated `permissions`), not a clean user-facing format.

**Discrepancies with prior docs:** DISCORD_COMMANDS_DOCUMENTATION.md is essentially correct here; neither doc mentions that the "user-only visibility" intent is not actually implemented.

**Rust target:** Keep as Administrator-only. Produce a JSON export of the guild's normalized data (guild row, trophies, user_trophies, guild_settings, role_rewards, leaderboard_panels) via SeaORM queries — not a raw DB dump. Send with Poise's ephemeral reply to actually fulfill the original intent. Build the JSON in memory (`serenity::CreateAttachment::bytes`), no temp files.

---

## /panel

**Purpose:** Create or delete the single persistent auto-updating leaderboard message for the guild.

**Definition:**
- Subcommands: `create`, `delete` (no options) (panel.js:10-19)
- Discord default permissions: `"32"` = Manage Guild (panel.js:7)
- Custom `permissions: ['manage_users']` → gated by imsafe (panel.js:5)
- Cooldown: none

### /panel create

**Current behavior (validated):**
1. Sends a plain message "Creating panel..." to the current channel (panel.js:31).
2. Stores `data.${guild}.panel = { message: msg.id, channel: channel.id }` (panel.js:32-35).
3. Calls `updatePanel(client, guild)` (panel.js:37) which edits that message into the leaderboard embed (globals.js:602-656).
4. Deletes the interaction reply on success (panel.js:38).
5. Any error in the whole block is swallowed by a bare `catch` and an error embed is shown (panel.js:39-47).

**updatePanel rendering (globals.js:602-656):** fetches the stored channel and message; includes users where `trophyValue` is truthy AND (member is in server OR `hide_quit_users == 1`) (globals.js:626); sorts descending; always renders page 1, 10 users, medals via `getMedal` (globals.js:111-119); "Total server score" is the sum over the *visible* users only (globals.js:628); name format from `leaderboard_format` setting via `parseFormat` (globals.js:149-167).

**Background refresh:** `updatePanels` (globals.js:685-696) loops forever: sleeps 60 s, then iterates every guild with a 1 s sleep between each. Panels are NOT updated when scores change — `updatePanel` is called only from `/panel create` and this loop (verified: no call in award.js/revoke.js/clear.js).

### /panel delete

**Current behavior (validated):**
1. Deletes `data.${guild}.panel` unconditionally (panel.js:51) — no check that a panel existed.
2. Replies success embed "Sucessfully **deleted** the panel." (panel.js:53-59).

**Validation rules & limits:**
- "One panel per guild" is enforced only implicitly: creating a second panel overwrites the DB record. Nothing prevents running create repeatedly.

**Data operations:**
- Write: `data.${guild}.panel` = `{ message, channel }` (create)
- Delete: `data.${guild}.panel` (delete)
- updatePanel reads: `data.${id}.panel`, `data.${id}.users`, `data.${id}.settings.hide_quit_users`, `data.${id}.settings.leaderboard_format` (via getSetting, globals.js:95-103)

**Edge cases, quirks and bugs:**
- BUG: Creating a new panel does not delete the previous panel message — the old message is orphaned in its channel forever (it just stops updating).
- BUG: `/panel delete` does not delete the panel message either; it only stops updates.
- QUIRK: If `updatePanel` fails after the DB write (e.g. missing embed perms), the panel record persists pointing at a raw "Creating panel..." message.
- QUIRK: Users with `trophyValue === 0` are excluded from panel and total because of the truthiness check (globals.js:626); negative scores are included.
- QUIRK: With ~2,500 guilds, a full `updatePanels` cycle takes ~42+ minutes (60 s + 1 s per guild), so "periodic" refresh is much slower than 60 s in practice.
- QUIRK: `/panel delete` reports success even when no panel existed.

**Discrepancies with prior docs:**
- CLAUDE.md: "Updates automatically when scores change" — FALSE; updates only via the background loop.
- CLAUDE.md: "Only one panel per server allowed" — not validated/enforced, only overwritten.

**Rust target:** Keep one panel per guild via the `leaderboard_panels` table (unique guild_id). On create: delete/replace the old panel message; on delete: attempt to delete the message. Update panels on score change (award/revoke/clear) plus a periodic reconciliation task. Compute the leaderboard on the fly with a SQL aggregate (scores are always computed, never stored).

---

## /permissions (perms.js) — DEPRECATED

**Purpose:** Legacy custom role-permission system; today it only shows a deprecation notice.

**Definition:**
- Subcommands still registered with options (perms.js:13-43): `add` (choices Manage Users/`manageusers`, Manage Trophies/`managetrophies`, Manage Rewards/`managerewards`; role option `target`), `remove` (same options), `list` (none).
- Discord default permissions: `"32"` = Manage Guild (perms.js:15)
- No `permissions` array → itself not gated by imsafe.

**Current behavior (validated):** For ALL subcommands and inputs, replies with a single red embed titled ":warning: Caution!" explaining the custom permission system is deprecated, linking Discord's Slash Commands Permissions blog post, warning that commands are disabled until `/imsafe` is run after setting Discord permissions ("every command that used this system will be available for everyone, beware!"), and linking the Support Server (perms.js:47-54). Unconditional `return` at perms.js:56; everything after (perms.js:58-200) is dead commented-out code and references undefined `Discord`/`parseName`/`checkName` — it could not run even if uncommented.

**Data operations:** none (dead code would have read/written `data.${guild}.permissions`).

**Edge cases, quirks and bugs:**
- QUIRK: The full option/choice UI is still exposed to users even though inputs are ignored.
- QUIRK: The dead code uses camel-less keys (`manageusers`) as choice values but snake_case (`manage_users`) in the DB — a naming inconsistency deliberately bridged by the dead code, not a mapping bug: choice values like `manageusers` are matched via `checkName` (perms.js:89,121,153) and read/write the snake_case DB keys `manage_users` etc. (perms.js:93-116).

**Discrepancies with prior docs:** both docs correctly say it only shows a deprecation notice. CLAUDE.md's command list still shows `/permissions add - Add permissions to a role.` etc. as if functional (marked DEPRECATED in one list, not in the other).

**Rust target:** Do NOT reimplement. Register `/permissions` (or drop it entirely) with only a static deprecation/informational reply pointing to Discord's native Integrations permissions. No table needed; do not migrate `data.${guild}.permissions`.

---

## /rewards

**Purpose:** Manage role rewards automatically granted when a user's score reaches a requirement.

**Definition:**
- Top-level description string is literally `'Create a new trophy for your server.'` (rewards.js:9) — wrong in source.
- Subcommands (rewards.js:10-33):
  - `add`: `role` (Role, required), `requirement` (Integer, required). Subcommand description is literally `'Add permissions to a role.'` — also wrong in source.
  - `remove`: `role` (Role, required)
  - `clear`: none
  - `list`: `page` (Integer, optional, default 1)
- Discord default permissions: `"32"` = Manage Guild (rewards.js:8)
- Custom `permissions: ['manage_rewards']` → gated by imsafe (rewards.js:5)
- Cooldown: none

### /rewards add

**Current behavior (validated):**
1. `requirement = Math.floor(Math.max(value || 0, 0))` (rewards.js:49).
2. Role must exist in the guild role cache, else "Role ... was not found." (rewards.js:51-60).
3. Role-hierarchy check (rewards.js:62-73) — dead, see BUG below.
4. Rejects requirement < 1 (rewards.js:75-83).
5. Rejects if existing rewards `length > 20` (rewards.js:85-94).
6. Duplicate check `a.requirement === requirement || a.role === role` (rewards.js:96-105) — role half broken, see BUG.
7. Pushes `{ role: role.id, requirement }`, sorts descending by requirement, saves (rewards.js:107-113).
8. Success embed (rewards.js:115-120).

### /rewards remove

**Current behavior (validated):**
1. Role must still exist in the guild cache (rewards.js:126-135).
2. Same dead hierarchy check (rewards.js:138-147).
3. Error if rewards list empty (rewards.js:149-158) or role not among rewards (rewards.js:160-168).
4. Filters the role out, re-sorts, saves (rewards.js:170-174).
5. Success embed with footer: the bot will NOT remove the role from members who already have it (rewards.js:176-182).

### /rewards clear

**Current behavior (validated):** errors if no rewards exist (rewards.js:216-225), else sets the array to `[]` (rewards.js:227) and confirms. Does not touch members' roles.

### /rewards list

**Current behavior (validated):** reads caller's stored score `data.${guild}.users.${author}.trophyValue` (?? 0) (rewards.js:188); builds `**:medal: {requirement}**\n<@&{role}>` lines; paginates 5 per page via `getPage` (rewards.js:201, globals.js:592-600, page clamped to [1, last]); embed titled `{guildName}'s Role Rewards` with "Your score" in the description (rewards.js:203-207). No page indicator in the footer. The embed field value is built as `'​' + pages.list.join('\n')` (rewards.js:207) — a zero-width space prefix — with each entry carrying its own trailing newline (rewards.js:198), so entries render double-spaced. With ZERO rewards the command still succeeds, rendering a field whose value is just the zero-width space. Rewrite note: Discord rejects empty embed field values (400), so the Rust version needs an explicit empty-state message instead of relying on the `​` trick.

**Role application (context, globals.js:169-235 `doRewardRoles`):** called from `/award`, `/revoke`, `/clear` only — never from `/rewards`. Requires bot MANAGE_ROLES; iterates rewards (sorted desc), grants the highest met reward, and with `stack_roles == 0` grants all met rewards, otherwise removes lower ones; removes rewards no longer met. `stack_roles` is read raw with `config?.stack_roles ?? 1` (globals.js:197), not via `getSetting`. Additions are applied BEFORE removals (globals.js:233-234: `member.roles.add(award)` then `member.roles.remove(remove)`), so a role appearing in BOTH lists ends up REMOVED. BUG (suppression): combined with the duplicate-role check bug at rewards.js:96, a duplicated reward role (7 guilds in production) landed in both lists whenever any of its tiers was met, and in the remove list otherwise — so it was effectively ALWAYS stripped, never held (under default `stack_roles == 1`). The Rust reward engine computes one final target role set per user, eliminating the ordering hazard.

**Validation rules & limits (as implemented):**
- Minimum requirement: 1 (after flooring/clamping to ≥0).
- Maximum rewards: check is `prev.length > 20` (rewards.js:86), so a 21st reward is allowed and the 22nd is blocked → effective max is 21, not 20.
- Duplicate requirement values are rejected; duplicate roles are NOT (see BUG).
- Role hierarchy: intended but never enforced (see BUG).

**Data operations:**
- Read/Write: `data.${guild}.rewards` (array of `{role: string, requirement: number}`, kept sorted descending by requirement)
- Read: `data.${guild}.users.${user}.trophyValue` (list)

**Edge cases, quirks and bugs:**
- BUG (hierarchy dead code): `if (check >= 0 && !interaction.guild.ownerId == interaction.user.id)` (rewards.js:65, 139). `!ownerId` is always `false`, and `false == user.id` is always `false`, so the condition never triggers — anyone with Manage Guild can add/remove rewards for roles above their own.
- BUG (duplicate role check): `a.role === role` compares a stored ID string to a Role object (rewards.js:96) — always false. The same role CAN be added multiple times with different requirements; only duplicate requirements are blocked.
- BUG (off-by-one): limit check allows 21 rewards (rewards.js:86).
- BUG (stuck rewards): `remove` requires the role to still exist in the guild (rewards.js:126-135); a reward pointing at a deleted role can only be removed via `/rewards clear`.
- QUIRK: removing/clearing rewards never retro-updates members' roles (explicitly stated in the remove footer); roles change only on the next award/revoke/clear of each user.
- QUIRK (probable v14 regression, globals.js:171-182): `guild.me` does not exist in discord.js v14 (fetch fallback saves it), and `permissions.has('MANAGE_ROLES')` uses a v13 flag name — in v14 this throws and the `catch` silently aborts `doRewardRoles`, meaning role rewards are never applied in the current deployment — EXCEPT in guilds where the bot has Administrator, since v14's `has()` short-circuits on Administrator before resolving the invalid flag string and role rewards work there (see core-behaviors.md).

**Discrepancies with prior docs:**
- The command description string bug is noted in DISCORD_COMMANDS_DOCUMENTATION.md, but CLAUDE.md reproduces "`/rewards add` - Add permissions to a role." as if it were the intended description.
- Both docs claim "Maximum 20 reward roles" — code allows 21.
- Both docs claim "Cannot add duplicate role or requirement" — duplicate role is not actually prevented.
- Both docs claim role-hierarchy enforcement ("User cannot add role higher than their highest role unless owner") — the check is dead code.
- DISCORD_COMMANDS_DOCUMENTATION.md's "rewards sorted by requirement (descending)" and "5 per page" are correct.

**Rust target:** `role_rewards` table (guild_id, role_id UNIQUE per guild, requirement, CHECK requirement >= 1; UNIQUE(guild_id, requirement) if we keep that rule). Enforce a true max of 20. Fix the hierarchy check (compare positions, bypass for guild owner). Allow removing rewards for deleted roles (operate on stored IDs). Apply/reconcile roles from computed on-the-fly scores whenever rewards change, and use correct Serenity permission checks so rewards actually work.

---

## /settings

**Purpose:** View and change the five per-guild configuration settings.

**Definition:**
- Subcommands (settings.js:11-24):
  - `set`: `setting` (String, required, choices generated from the settings array) and `value` (String, optional).
  - `list`: none.
- Discord default permissions: `"32"` = Manage Guild (settings.js:9)
- No `permissions` array → NOT gated by imsafe.
- Cooldown: none

**Validated settings table (globals.js:52-88)** — stored value is the 0-based option index:

| id | Name | Options (index 0, 1, 2, 3) | Real default |
|---|---|---|---|
| `dedication_display` | Dedication Display | Always Mention, Always Name, Mention Only in Server | 2 (Mention Only in Server) |
| `stack_roles` | Stack Roles | Stack Roles, Only Highest Reward | 1 (Only Highest Reward) |
| `hide_unused_trophies` | Hide Unused Trophies | Hide Unused Trophies, Show Unused Trophies | 1 (Show Unused Trophies) |
| `hide_quit_users` | Hide Quit Users | Hide Quit Users, Show Quit Users | 0 (Hide Quit Users) |
| `leaderboard_format` | Leaderboard Format | Mention, Username, Nickname, Username and Tag | 0 (Mention) |

`getSetting` (globals.js:95-103) returns the stored index or the default when unset (`config ?? stg.default`, so a stored 0 is respected).

### /settings list

**Current behavior (validated):** reads `data.${guild}.settings` once (settings.js:37); for each setting shows `stored ?? default` resolved to its option label, the description, and all option labels (settings.js:38-53); embed titled ":gear: {guild}'s Settings" with footer usage hint (settings.js:55-58).

### /settings set

**Current behavior (validated):**
1. Reads `setting`; validation `if (!setting && !available.includes(setting))` (settings.js:68) — see BUG.
2. `value` defaults to `object.default + 1` when omitted (settings.js:78) because numeric input is 1-based; omitting the value resets the setting to its default.
3. `findOption` (settings.js:102-119): if `parseInt(value)` is a number, treats it as a 1-based option number and validates range; otherwise normalizes with `parseName` (lowercase, strip non-word chars, globals.js:237-240) and returns the first option whose normalized label CONTAINS the input (`checkName`, globals.js:242-244, via `findIndex`).
4. On match, writes the 0-based index to `data.${guild}.settings.${setting}` (settings.js:83) and confirms with the resolved label; otherwise "You must specify a valid option for this setting." (settings.js:88-93).

**Validation rules & limits:** setting must be one of the five (enforced in practice by Discord choices); value must resolve to a valid option number (1..N) or a substring-matching option name.

**Data operations:**
- Read: `data.${guild}.settings` (list), Write: `data.${guild}.settings.${setting}` = index (set)

**Edge cases, quirks and bugs:**
- BUG (latent): the guard uses `&&` instead of `||` (settings.js:68), so an unknown non-empty setting id would pass. The crash site depends on the input: with `value` OMITTED it crashes on `object.default` at settings.js:78; if a crafted API call supplies an unknown setting WITH a value, the `?? (object.default + 1)` short-circuits and the crash occurs inside `findOption`: with a NUMERIC value at settings.js:107 (`n < object.options.length`); with a NON-numeric value the isNumber branch is skipped and the crash is at settings.js:114 (`object.options.findIndex(...)`). Unreachable through the Discord UI because of choices, but reachable via crafted API calls.
- QUIRK: name matching is substring-based and first-match-wins — e.g. `value:"mention"` on Leaderboard Format matches option 1 "Mention", but on Dedication Display it matches "Always Mention" (index 0), not "Mention Only in Server".
- QUIRK: numeric values are 1-based for users while 0-based indexes are stored; `parseInt("2abc")` is accepted as 2.
- QUIRK: not imsafe-gated even though it is a management command.

**Discrepancies with prior docs:**
- CLAUDE.md's "Guild Settings" bullet list is misleading/wrong: it phrases `stack_roles` as "0=Stack All, 1=Only Highest", `hide_unused_trophies` as "0=Hide, 1=Show", `hide_quit_users` as "0=Hide, 1=Show" without defaults, while elsewhere implying different behavior; the validated defaults are the table above (2, 1, 1, 0, 0). DISCORD_COMMANDS_DOCUMENTATION.md's "Global Settings Reference" matches the code.
- CLAUDE.md claims "'imsafe' mode required for management commands" — `/settings` (and `/export`) are not gated.

**Rust target:** `guild_settings` table with typed columns (or key/index pairs) seeded with the validated defaults above; treat missing rows as defaults exactly like `getSetting`. Use Poise choice parameters for both setting and value (per-setting value choices or an autocomplete), eliminating the 1-based/substring parsing quirks. Keep "omit value = reset to default".
