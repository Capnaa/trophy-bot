# Trophy Bot — Core & Event System: Validated Behavior Spec

Source of truth: the JavaScript code at `globals.js`, `index.js` and `events/*.js`
(discord.js 14.21.0 as resolved by package-lock.json — what `npm ci` deploys; package.json
declares `^14.6.0` — quick.db 7.1.3), read completely and validated on 2026-07-07.
The prior markdown docs (CLAUDE.md, DISCORD_COMMANDS_DOCUMENTATION.md,
COMMANDS_AND_FUNCTIONALITY.md) were AI-generated and contain errors; where they
disagree with the code, this document wins. `TrophyBot-Copy/` is a backup and was ignored.
The superseded documents referenced in the discrepancy sections live in `docs/archive/`.

## Client & startup

`index.js`:

- Creates a `Discord.Client` with intents: `GuildMessages`, `GuildMembers`,
  `GuildEmojisAndStickers`, `Guilds` (index.js:7-14). No message-content intent; the bot
  never reads messages, so `GuildMessages`/`GuildEmojisAndStickers` are unnecessary.
- `client.version = 0` (index.js:16) — stored into bot data on first init.
- Database: `client.db.bot = new db.table('bot')`, `client.db.guilds = new db.table('guilds')`
  (quick.db tables backed by `json.sqlite`; everything under path prefix `data.`).
- Loads every `events/*.js` file; each exports `{name, once, run}` and is bound with
  `client.on`/`client.once` (index.js:36-55). Two files both bind `interactionCreate`
  (`button.js` and `command.js`); each filters by interaction type.
- `dotenv` config, then `client.login(process.env.DISCORD_TOKEN)`.

`events/ready.js` (once):

1. Fetches `client.errorChannel` = channel `985869722199416862` and
   `client.suggestionChannel` = channel `985872094153830400` (ready.js:11-15). Both
   fetches share a single try/catch, so a failed `errorChannel` fetch leaves it
   **undefined** (not null) *and* skips the `suggestionChannel` fetch — see dispatch
   step 8 for the fallout. `suggestionChannel` is never used anywhere.
2. Loads commands via `fetchModules('../commands', '.js', true)` and languages from
   `../locale/languages` into `client.commands` / `client.languages` (languages are
   effectively unused — the language system is disabled).
3. Calls `fixShit(client)` (currently a no-op, see below).
4. `client.cooldowns = new Discord.Collection()` — created but **never read or written
   anywhere else**; cooldowns are non-functional (see Bugs).
5. Sets an initial activity using the v13-style signature
   `setActivity(name, { type: 'WATCHING' })` (ready.js:32) — invalid under v14 enums;
   harmless because `changeActivity` replaces it seconds later with a proper
   `ActivityType.Watching`.
6. Spawns `changeActivity(client)` and `updatePanels(client)` (fire-and-forget infinite
   loops), then `await AttemptToFetchUsers(client, true)` (full member fetch of every guild).
7. **Last**, initializes bot data if `data` key missing (ready.js:41-53):
   `{version, defaultLanguage: 'en', bannedUsers: [], commands: {total: 0}, trophies: 0,
   trophiesAwarded: 0}`. Note this runs *after* steps that read bot data (they survive via
   `?? 0` fallbacks).

Command registration (inside `fetchModules`, globals.js:335-354): if `DEBUG == "true"`,
commands are PUT as **guild commands** for each ID in `testingServers`
(`985439832388042822`, `1393760778972041258`); otherwise PUT as **global application
commands**. Uses `@discordjs/rest` with API v9 routes.

## Command dispatch & permission flow

`events/command.js` — for every `interactionCreate` that `inGuild()` and `isCommand()`:

1. `await interaction.deferReply()` — **always public** (command.js:14). Commands that later
   pass `ephemeral: true` to `editReply` (invite, support) are not actually ephemeral;
   Discord ignores the flag on edit of a public deferral.
2. `getServer(client, guild.id, guild)` — fetches the guild and lazily creates the guild
   DB record (see Shared functions; note it creates new guilds with `imsafe: true`).
3. Fire-and-forget `AttemptToFetchUsers(client)` (daily member-cache refresh trigger).
4. Looks up the command in `client.commands`; silently returns if missing.
5. Fetches the invoking member; computes `roles` (role-ID array) and `isAdmin` — **both
   dead code**, never used. The `isAdmin` check uses the v13 string `'ADMINISTRATOR'`
   against v14 `toArray()` output (`'Administrator'`), so it would always be false anyway
   (command.js:36-37). Dead or not, the `isAdmin` computation still costs a live
   `client.channels.fetch()` on every dispatch (command.js:37) — and that fetch runs
   before the try/catch, so its rejection is a process-crash hazard (see Bug #1).
6. **imsafe gate** (command.js:39-44): if the command module declares a `permissions`
   array AND the invoker is not the dev (`isDev`), read
   `data.${guild}.imsafe ?? false`; if falsy, reply with `imsafeWarning` and stop.
   Commands with a `permissions` property: `award`, `clear`, `create`, `delete`, `edit`,
   `panel`, `revoke`, `rewards` (values `manage_users` / `manage_trophies` /
   `manage_rewards` — the values themselves are never checked; only the property's
   existence matters). Real authorization is Discord-side via each command's
   `setDefaultMemberPermissions` ("8" or "32").
7. Stats counters: ensures `data.commands.${name}` exists, then `add` 1 to it and to
   `data.commands.total` (command.js:46-49). Counted **before** execution, so failed
   commands still count.
8. `await command.run(interaction)` inside try/catch. On error:
   - `console.error`, then if `client.errorChannel !== null` (command.js:57), sends an
     embed there with the command string, "perpetrator" user ID, guild ID, and the first
     900 chars of the stacktrace (command.js:57-76). If *that* send fails, posts
     "Error log could not be sent, dev should know about this..." to the invocation
     channel (command.js:72-75). **The guard is `!== null`, but ready.js leaves
     `client.errorChannel` undefined when its fetch throws — undefined passes the
     check.** Net effect: when the error channel is unreachable (e.g. running the
     legacy bot locally during migration testing), every command error publicly posts
     the "Error log could not be sent..." message into the invoking channel.
   - Edits the reply with a red embed pointing to the support server, footer noting
     errors are auto-delivered to the developer (command.js:81-86).
9. **No cooldown enforcement exists.** `cooldown: 10` is declared on `stats` and `suggest`
   modules and `showCooldown()` exists in globals, but no code ever checks them.

`events/button.js` — for every button `interactionCreate` in a guild:

1. `deferReply({ ephemeral: true })`.
2. If the clicker is the **guild owner**:
   - `customId == "forgetmeproceed"` → replies "Okay! Thanks for using Trophy Bot...",
     then `forgetMe(client, guild)` (delete data + leave). The only button the bot ever
     creates (in `/forgetme`).
   - `customId == "forgetmenope"` → "Operation stopped". **Dead branch**: no such button
     is ever created.
3. Fallback: any other button / non-owner clicker gets the literal ephemeral text
   `"Not replied"` (button.js:28-32).
4. **There is no button-based pagination.** All pagination is via a `page` integer option
   on the commands themselves.

## Events

- `guildCreate` (`join.js`): logs, `guild.members.fetch()` (full member fetch on join).
  One-off milestone: when cache size hits exactly 75 guilds and `data.milestone` is unset,
  pings `<@353998390734094346>` in the error channel and sets `data.milestone = true`
  (already `true` in production). **No guild record is created on join** — records are
  created lazily on first command via `getServer`.
- `guildDelete` (`leave.js`): `console.log("Left a guild!")` only. No data cleanup, no
  tracking.
- `guildMemberAdd` (`user.js`): support-server-only welcome role. If the member is not a
  bot and the guild is `985439832388042822`, adds role `985440033286787123`. The
  `member.roles.add()` (user.js:16) is **not awaited**, so the surrounding try/catch does
  NOT catch its rejection — it escapes as an unhandled promise rejection (see Bug #1's
  crash-surface note). Nothing to do with role rewards.
- `interactionCreate`: handled twice (command.js + button.js, above).

## Background tasks

| Task | Where | Schedule |
|---|---|---|
| `changeActivity` | spawned in ready | "Starting up!" → sleep 10 s → infinite loop: set a random activity, sleep 60 s (globals.js:449-460) |
| `updatePanels` | spawned in ready | infinite loop: sleep 60 s, then `updatePanel` for every cached guild with 1 s sleep between guilds; per-guild errors swallowed (globals.js:685-696) |
| `AttemptToFetchUsers` | ready (forced) + every command dispatch | Compares `new Date().getDate()` (day of month!) to stored `data.lastDay`; when different (or forced), stores today and does `guild.members.fetch()` for **all** guilds (globals.js:482-496) |

Random activities (globals.js:462-480): awarded-trophy count, created-trophy count, total
commands, guild count, cached-user count, uptime (`timeFormat`), "Invite me with
'/invite'", "We are the champion"; on any error, "Working harder than expected!".

## Shared functions

All line references are `globals.js`.

### getTrophy(client, guild, trophy) → key | null (:121-147)

Resolution order:

1. **ID lookup**: if `parseInt(trophy)` is not NaN (i.e. the input merely *starts* with
   digits), check `data.${guild}.trophies.${trophy}` using the **raw full input string**
   as the key. Exact key hit → return the input unchanged. (So `"3rd place"` never falsely
   hits ID 3, but a trophy *named* `"12"` is shadowed by trophy ID 12 if it exists.)
2. **Name lookup**: normalize the query with `parseName` (lowercase, strip every `\W`
   char — spaces, punctuation, emoji; underscores survive). Iterate all trophy keys
   (skipping `current`); JS numeric-string keys iterate in **ascending numeric order**, so
   the **lowest trophy ID wins** among duplicates. Match rule (`checkName`, :242-244):
   normalized *stored name* `.includes(` normalized *query* `)` — i.e. **case- and
   punctuation-insensitive SUBSTRING match**, not exact. Query `"gold"` matches
   "Golden Medal".
3. No match → `null`.

Edge case: a query that normalizes to the empty string (e.g. `"!!!"` or an emoji) matches
the **first** trophy, since `"anything".includes("") === true`.

### getSetting(client, guild, setting) → number|null (:95-103) and settings (:52-88)

Returns the stored `data.${guild}.settings.${id}` or the default (`??`, so a stored `0`
is honored). Unknown setting id → `null`. Values are indices into `options`.

| id | options (index order) | default |
|---|---|---|
| `dedication_display` | Always Mention, Always Name, Mention Only in Server | **2** |
| `stack_roles` | Stack Roles, Only Highest Reward | **1** |
| `hide_unused_trophies` | Hide Unused Trophies, Show Unused Trophies | **1** (Show) |
| `hide_quit_users` | Hide Quit Users, Show Quit Users | **0** (Hide) |
| `leaderboard_format` | Mention, Username, Nickname, Username and Tag | **0** (Mention) |

### doRewardRoles(client, guild, id) (:169-235)

Intended algorithm (rewards are stored **sorted descending by requirement** by
`/rewards add|remove`):

1. Uses `guild.me` (v13 API; `undefined` in v14) with a fetch fallback, then checks
   `me.permissions.has('MANAGE_ROLES')` inside try/catch. **In discord.js v14 the flag
   name is `ManageRoles`; `'MANAGE_ROLES'` throws `BitFieldInvalid`, the catch returns,
   and the whole function is a silent no-op — EXCEPT in guilds where the bot has
   Administrator, since v14's `has()` short-circuits on Administrator before resolving
   the flag string and the function then runs fully.** Role rewards are dead in
   production under v14 in all other guilds (see Bugs).
2. Were the check passing: walk rewards high→low against `user.trophyValue`:
   - While `score < requirement`: remember the role; each subsequent iteration pushes the
     previously remembered (unearned) role into `remove` — so all roles above the earned
     tier end up removed.
   - First reward with `score >= requirement`: push to `award` (the "best" role), remove
     the last unearned role, set `foundBest`.
   - Remaining (lower) rewards: if `stack_roles == 0` push to `award` (stacking),
     else push to `remove`.
   - If no tier reached, the last remembered role is removed too (net effect: every
     reward role removed).
3. `member.roles.add(award)` then `member.roles.remove(remove)`, reason "Role rewards".
   No hierarchy filtering here — a reward role above the bot's top role would make the
   add call reject (hierarchy is only validated at `/rewards add` time).
4. Reads `stack_roles` directly from `data.${guild}.settings` with `?? 1` (bypasses
   `getSetting`, same default).

### getPage(list, perPage, page = 1) → {list, last, page} (:591-600)

`last = ceil(len/perPage)`; `page` clamped to `[1, last]`; returns the slice, the clamped
page, and `last` floored at 1. Empty list → `{list: [], last: 1, page: 1}`.

### parseUser(client, ref, notfound = null, guild = null, member = false) (:507-544)

- Empty ref → `notfound`. Strips `<@`, `<@!`, `>` from mentions.
- If the result is a positive integer < 2^63 (`isOnSnowflakeRange`, :546-553 — note this
  accepts tiny numbers like `"5"`): `client.users.fetch(id)` **globally** (user need not
  be in the guild); fetch failure → `notfound`.
- Otherwise, if a guild was passed and is available: `guild.members.search({query, limit: 1})`
  (prefix username search). No guild → `notfound`.
- If the found object has a `.guild` differing from the passed guild → `notfound`.
- `member == false` (default) unwraps a GuildMember to its User.

### downloadImage(url, filename) (:500-504)

`node-fetch` GET → arrayBuffer → `fs.writeFile` (promisified). No size/type validation
here (callers validate), no error handling — returns the promise.

### getServer(client, id, guild) (:405-447)

`await guild.fetch()` on **every call** (i.e. every command). No id → `{language: 'en'}`.
If `data.${id}` exists and is not `-1` (the `forgetMe` tombstone), returns it. Otherwise
creates and stores:

```js
{ id, language: 'en', settings: {}, trophies: { current: 0 }, users: {}, rewards: [],
  permissions: { manage_trophies: [], manage_users: [], manage_rewards: [] },
  imsafe: true, restapi: { token: '', enabled: false } }
```

Note `imsafe: true` — new (and post-forgetme re-created) guilds never see the imsafe
warning. `restapi` is vestigial.

### getDedication(guild, dedication, config) (:33-49)

`dedication = {user, name}`. No name → `null`; no user id → the name.
`config 0` (Always Mention) → `<@id>`. Otherwise fetch the member (errors swallowed);
not in guild → name. `config 1` (Always Name) → `user?.username ?? name` — but a
GuildMember has no `.username` (it is `member.user.username`), so this **always falls
back to the stored name**. Any other config → `<@id>`.

### getTrophyCount(client, guild) (:246-250)

Own-property count of `data.${guild}.trophies` minus 1 (for the `current` counter key).
Missing trophies object → `{}` → returns **-1**.

### cleanseTrophies(client, guild, trophy, value) (:252-265)

For every user: repeatedly splice the trophy key out of `user.trophies` and subtract
`value` per removal from `user.trophyValue`, then write the whole users object back.
Uses the trophy's *current* value, so scores drift if the value ever changed after
awarding (scores are stored denormalized, never recomputed).

### isInServer(guild, user) (:90-92) / attemptFetchIfCacheCleared (:283-288)

`isInServer` is a pure member-**cache** check. `attemptFetchIfCacheCleared(keys, guild)`
does a full `guild.members.fetch()` when the DB user-key count exceeds the cache size —
a heuristic used before leaderboard/panel rendering so `isInServer` is meaningful.

### anyIn(from, which) (:267-269)

`which.some(x => from.includes(x))` — any element of `which` present in `from`.

### isDev(id) (:388-390) / isBanned (:393-395)

`isDev`: exactly one hardcoded developer ID: `'353998390734094346'`.
`isBanned`: always `false` (the `bannedUsers` DB array is never consulted).

### showError / showSuccess / showCooldown / imsafeWarning (:567-579, :21-31)

- `showError(msg)` → `` `${customErrorEmoji} **Oopsie!** ${msg}` ``.
- `showSuccess(msg)` → `` `✅ **Great!** ${msg}` ``.
- `showCooldown(t)` → "Calm down!..." — defined but **never called**.
- `imsafeWarning(interaction)`: edits the reply with the red deprecation warning embed
  (lists the 8 formerly-custom-permission commands, links the support server).

### fetchModules(dir, ext='.js', command=false, first=true) (:290-365)

Recursive loader. Skips files starting with `-`; treats any name without a `.` as a
directory. Command mode requires `module.data.name` (SlashCommandBuilder); non-command
mode requires `module.names` (locale files) and keys by `names[0]`. After loading, the
top-level call registers slash commands with Discord (see Client & startup). The final
`collection.sort(a => a.name)` uses a comparator returning a string — effectively
meaningless.

### fixShit(client) (:658-683)

Currently logs "Fixing..." / "Fixed" and nothing else; the body is commented-out one-off
data surgery for a specific guild (`1316734441187577966`). It is NOT database
initialization (that lives at the end of ready.js). No migration logic to port.

### changeActivity / updatePanels / updatePanel / AttemptToFetchUsers

Schedules covered under Background tasks. `updatePanel(client, guild)` (:602-656):

1. Reads `data.${id}.panel` (`{message, channel}`); missing → return. Fetches channel and
   message (a rejected fetch throws — swallowed by `updatePanels`, so stale panels are
   retried forever and never cleaned from the DB).
2. Builds page 1 of the leaderboard: users with truthy `trophyValue` (**exactly-0 scores
   are excluded**; negative scores are truthy and shown) where `isInServer` OR
   `hide_quit_users == 1` (Show). Sorts by value descending,
   `getPage(keys, 10, 1)`.
3. `total` = sum of the included users' values; header "Total server score: **N** :medal:".
4. Each row: `getMedal(i)` (🥇🥈🥉 for ranks 1-3, `:medal:` otherwise) +
   `**{i}.-** {parseFormat(...)} ➤ **{value}** :medal:`. `parseFormat` (:149-167):
   0 → mention; 1 → username; 2 → nickname ?? username; 3 → `user.tag`; formats 1-3 do an
   uncaught `guild.members.fetch(id)` that **rejects for quit users**, aborting the panel
   update when quit users are shown.
5. Edits the panel message (content zero-width space + embed titled
   `🏆 {guild}'s Leaderboard`).

### forgetMe(client, guild) (:367-385)

Deletes each trophy's image file from `./images/`, sets `data.${guild.id} = -1`
(tombstone; `getServer` recreates fresh data if the guild comes back), and
`guild.leave()`. Errors logged, not surfaced.

### Misc

`timeFormat(ms)` → `"Xh Ym Zs"` (:271-281). `clamp` (:581-584). `clearMentions`
(:559-564) — zero-width-space escapes `@everyone/@here/<@`. `isAlphanumeric` (:586-589).
`sleep(ms)` (:555-557 — references an undefined `DEF_DELAY` if called with no arg).
`booleans` (:698-701) — yes/no string aliases, exported for settings parsing.

## Constants & limits

- Colors (:15-19): main `#0096FF`, error `#E02D44`, success `#32CD32`.
- Emoji (:105-109): trophy 🏆, success ✅, error = custom emoji `<:error:985469320844967946>`.
- Medals (:111-119): 🥇/🥈/🥉 for ranks 1-3, `:medal:` otherwise.
- Support server invite: `https://discord.gg/kNmgU44xgU`; support server ID `985439832388042822`.
- Invite URL (commands/bot/invite.js:15):
  `https://discord.com/oauth2/authorize?client_id=985134052665356299&permissions=34816&scope=applications.commands%20bot`.
- Testing servers (:399-402): `985439832388042822`, `1393760778972041258`.
- Error-log channel `985869722199416862`; suggestion channel `985872094153830400` (unused);
  support-server welcome role `985440033286787123`.
- Dev ID: `353998390734094346`.
- Pagination: 10 per page (leaderboard, trophy lists, panel); rewards list 5 per page.
- Other limits (150 trophies/guild, effectively 21 rewards — rewards.js:86 checks `> 20`
  *before* pushing, see the commands-admin.md off-by-one finding — 1-50 award count,
  field lengths, 1 MB images) live in the individual commands, not in globals.

## Bugs & quirks found

1. **BUG — role rewards are dead in most guilds under discord.js v14.**
   `doRewardRoles` checks `me.permissions.has('MANAGE_ROLES')` (v13 flag name);
   `has()` throws `BitFieldInvalid` and the catch silently returns
   (globals.js:177-182). EXCEPTION: v14's `has()` short-circuits on Administrator
   before resolving the flag string, so in guilds that granted the bot an
   Administrator role the function executes fully and role rewards WORK. Verified
   empirically on the deployed discord.js 14.21.0 (the package-lock.json resolution
   installed by `npm ci`; package.json declares `^14.6.0`) — the `has()` short-circuit
   behavior is identical in 14.6 and 14.21, so the Administrator-exception claims stand
   for either version. The stock invite URL grants
   neither ManageRoles nor Administrator, so default-invited guilds have dead
   rewards. Additional hazard: doRewardRoles is invoked WITHOUT await inside
   try/catch from award/revoke/clear, so in Administrator guilds a rejected
   `roles.add`/`roles.remove` (e.g. role above the bot) becomes an unhandled
   promise rejection, which crashes the process under Node 18 defaults.
   (`guild.me` is removed in v14, but the fallback
   `guild.members.fetch(client.user.id)` works.)
   The crash surface is wider than doRewardRoles: **no event handler has a top-level
   catch** — index.js:36-55 binds `(...args) => event.run(...args)` bare — so any
   rejection outside a handler's own try/catch also crashes the process under Node 18
   defaults. Concretely: events/command.js:14 `deferReply()` (expired or double-acked
   interaction), command.js:21 → getServer's unconditional `guild.fetch()`
   (globals.js:407), command.js:29 `interaction.member.fetch()`, and command.js:37's
   dead-code `client.channels.fetch()` all run BEFORE the try/catch that wraps only
   `command.run`; events/user.js:16 `member.roles.add()` is not awaited, so its
   surrounding try/catch never sees the rejection; events/join.js:11
   `await guild.members.fetch()` has no catch at all. (Note the dead-code `isAdmin`
   computation still costs a live channel fetch on every command dispatch.) Rust
   target: every event/dispatch path must be wrapped — already reflected in
   rust-parity-plan.md §2.
2. **BUG — cooldowns are never enforced.** `cooldown: 10` on stats/suggest,
   `client.cooldowns` collection, and `showCooldown()` all exist; nothing checks them.
3. **BUG — "Always Name" dedication never uses the live username**: `user?.username` on a
   GuildMember is undefined; always falls back to stored dedication name (globals.js:47).
4. **BUG — panel update crashes on quit users** when `leaderboard_format` is 1-3 and quit
   users are shown: uncaught rejected `members.fetch` in `parseFormat` aborts the edit
   (silently, via updatePanels' catch).
5. **QUIRK — getTrophy is a substring match**, case/punctuation-insensitive, lowest ID
   wins on duplicates; a symbols-only query matches the first trophy; a trophy named like
   a number is shadowed by that trophy ID.
6. **QUIRK — new guilds get `imsafe: true`** (`getServer`), and `/forgetme` + re-invite
   also resets it to true; the imsafe gate only ever affects pre-1.4 legacy guild records.
7. **QUIRK — ephemeral replies are not ephemeral**: command.js always defers publicly;
   `ephemeral: true` on later `editReply` is ignored.
8. **QUIRK — command counters increment before execution**, so failed runs are counted
   (imsafe-blocked runs are not — gate returns first).
9. **QUIRK — dead code in dispatch**: `roles` and `isAdmin` computed and never used;
   `isAdmin` also uses the v13 `'ADMINISTRATOR'` string (always false in v14).
10. **QUIRK — `forgetmenope` button handler exists but the button is never created**; any
    other/non-owner button click gets the literal ephemeral text "Not replied".
11. **QUIRK — no pagination buttons exist anywhere**; pagination is purely a `page`
    command option.
12. **QUIRK — `AttemptToFetchUsers` compares only day-of-month** (`getDate()`), and is
    fired on every command dispatch; full member fetch of all guilds once per "day".
13. **QUIRK — bot data initialized at the END of ready**, after tasks that read it;
    `fixShit` is a no-op (commented-out one-off patches), not initialization.
14. **QUIRK — `getTrophyCount` returns -1** for a guild with no trophies object.
15. **QUIRK — scores are stored denormalized** (`trophyValue`) and only ever
    incrementally adjusted; `cleanseTrophies` subtracts the trophy's *current* value, so
    drift is possible and never reconciled.
16. **QUIRK — stale panels are never cleaned up**: fetch failures are swallowed each
    minute forever; the DB record persists.
17. **QUIRK — users with exactly 0 score are omitted from the panel/leaderboard** (truthy
    check on `trophyValue`); negative scores are shown.
18. **QUIRK — `parseUser` treats any positive integer string as a snowflake** and fetches
    globally, so dedications can reference users not in the guild.
19. **QUIRK — vestigial data**: `getServer` seeds unused `language`, `permissions` arrays
    and a `restapi {token, enabled}` block; `isBanned` always false (`bannedUsers` unused);
    `suggestionChannel` fetched but unused; `fetchModules`' final sort is a no-op;
    `sleep()` without args references undefined `DEF_DELAY`.
20. **QUIRK — guildDelete does nothing** (log only); guild data is only removed via
    `/forgetme`. `guildCreate` full-fetches members and contains a hardcoded, already-fired
    75-server milestone ping.

## Discrepancies with prior docs

- Docs claim 10 s cooldowns on `/stats` and `/suggest` are enforced — they are declared
  but never enforced.
- CLAUDE.md claims "Button Interactions: For paginated embeds" — no pagination buttons
  exist; `button.js` handles only the forgetme confirmation.
- CLAUDE.md says `isDev` checks "hardcoded user IDs" (plural) — there is exactly one:
  `353998390734094346`.
- CLAUDE.md's `parseUser(client, input, fallback, guild, fetchFromDiscord)` — the 5th
  parameter is actually `member` (return GuildMember vs User), and lookup is
  snowflake-fetch or guild username search, not "mentions/IDs/names" generically.
- CLAUDE.md claims `fixShit` performs "database initialization and migration" — it is a
  no-op; init lives in ready.js.
- CLAUDE.md claims `AttemptToFetchUsers` "update[s] user count statistics" — it only
  refreshes member caches (daily); the only stat written is `lastDay`.
- Docs claim "Bot developers bypass all restrictions" — the dev bypass only skips the
  imsafe gate; Discord-side permission checks still apply.
- Docs claim `GuildCreate/Delete: Track bot joins/leaves` — nothing is tracked; leave is
  a console.log, join is a member fetch plus a dead milestone easter egg.
- Docs claim "User Updates: For role reward automation" — `user.js` only assigns a
  welcome role in the support server; role rewards are (nominally) applied inline by
  award/revoke/clear via `doRewardRoles`.
- No doc mentions that `getTrophy` name resolution is a normalized **substring** match
  with lowest-ID-wins, nor the empty-normalization wildcard behavior.
- No doc mentions that role rewards are entirely broken under the current discord.js v14
  dependency, that `getServer` seeds `imsafe: true`, or that public deferral defeats
  ephemeral replies.
- Settings defaults in DISCORD_COMMANDS_DOCUMENTATION.md **match the code** (2/1/1/0/0) —
  confirmed correct.

## Rust target notes

- **Error containment**: every event/dispatch path must be wrapped in a top-level catch —
  the JS bot binds handlers bare and crashes on any stray rejection (Bug #1's
  crash-surface note). Already reflected in rust-parity-plan.md §2.
- **Poise replaces**: `fetchModules` + REST registration (framework command registration),
  the dispatch/permission flow (`default_member_permissions`, `guild_only`), cooldowns
  (declare 10 s on stats/suggest and make them real), and error hooks (`on_error` →
  reproduce the error-channel log + user-facing embed).
- **Do not port**: custom permission system, imsafe gate + `imsafeWarning` (all guilds are
  effectively safe; `/imsafe` can become a friendly no-op or be dropped), language system,
  `fixShit`, `restapi` fields, `isBanned`, milestone easter egg, `suggestionChannel`.
- **getTrophy**: the target design uses per-guild unique names + autocomplete. Decide
  deliberately whether to keep substring matching inside autocomplete (good UX) while
  resolving the final value exactly; do NOT reproduce the empty-string wildcard or the
  numeric-ID shadowing (UUIDv7 IDs are internal only).
- **doRewardRoles**: reimplement from the *intended* algorithm above (fixed): check
  `ManageRoles`, skip roles at/above the bot's top role instead of failing the whole call,
  honor `stack_roles`, and remove-all when below every tier. Since scores are computed on
  the fly in Rust, call it after any award/revoke/clear inside the same flow.
- **Scores**: computing on the fly removes bug class #15 permanently; the panel/leaderboard
  0-score exclusion should become an explicit `HAVING score != 0` decision (document it).
- **Panels**: keep the 60 s refresh but delete/disable the panel record after N
  consecutive fetch failures (fixes quirk #16); handle quit users without throwing
  (fixes bug #4).
- **Member caching**: replace `AttemptToFetchUsers`/`attemptFetchIfCacheCleared` heuristics
  with targeted member fetches (Serenity chunking or per-page `members.fetch`) — do not
  port the daily full-fleet fetch.
- **Ephemeral**: with Poise, defer per-command (`ephemeral` where intended) instead of a
  blanket public defer.
- Preserve exact user-visible strings where continuity matters: colors, medal/row format
  (`{medal} **{i}.-** {user} ➤ **{value}** :medal:`), "Total server score", error embed
  wording, support/invite links, activity rotation texts.
