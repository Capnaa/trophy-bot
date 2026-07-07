# Utility Commands — Validated Specification

Validated against the actual JavaScript source (`commands/bot/*.js`, `events/command.js`, `events/button.js`, `globals.js`) on 2026-07-07. The JS code is the only source of truth; where DISCORD_COMMANDS_DOCUMENTATION.md or CLAUDE.md disagree with the code, the code wins and the discrepancy is noted. `TrophyBot-Copy/` was ignored.

## Shared dispatcher behavior (applies to all commands below)

All slash commands run through `events/command.js`:

1. DM interactions are ignored (`events/command.js:9`).
2. The reply is **always deferred non-ephemerally** (`events/command.js:14`). Consequence: no command in this bot can actually be ephemeral, because `editReply` cannot change the visibility of an already-deferred public reply (see `/invite`, `/support` below).
3. Guild data is initialized via `getServer` (`events/command.js:21`, `globals.js:405`). Note: **new guilds are created with `imsafe: true`** (`globals.js:437`), so `/imsafe` only matters for guilds predating v1.4.
4. Daily member-cache refresh via `AttemptToFetchUsers` (`events/command.js:24`, `globals.js:482`): fetches all members of all guilds once per calendar day-of-month (`data.lastDay`).
5. If the command module declares a `permissions` property and the caller is not the hardcoded dev (`globals.js:388-390`, ID `353998390734094346`), the guild's `imsafe` flag is checked; if false, `imsafeWarning` is shown instead of running the command (`events/command.js:39-44`, `globals.js:21-31`). None of the utility commands declare `permissions`, so none are gated.
6. Per-command and total run counters are incremented **before** execution (`events/command.js:46-49`): `data.commands.${name}` and `data.commands.total`. Counted even if the command later throws.
7. Errors are caught, logged to a hardcoded developer channel (`events/command.js:57-76`, channel fetched at `events/ready.js:12`), and a generic error embed is shown.

**QUIRK (dispatcher):** `cooldown` properties on command modules are never enforced. `client.cooldowns` is created (`events/ready.js:28`) but no code ever reads it. `showCooldown` (`globals.js:577`) is dead code.

**QUIRK (dispatcher):** `events/command.js:36-37` computes `roles` and `isAdmin` but never uses them; `isAdmin` also checks the string `'ADMINISTRATOR'`, which is not a valid discord.js v14 permission name (v14 uses `Administrator`), so it would always be false anyway.

---

## /about

**Purpose:** Static informational embed about the bot (links, creator credit).

**Definition** (`commands/bot/about.js:5-7`)
- Parameters: none. Default permissions: none. Cooldown: none. Not ephemeral (public).

**Current behavior (validated)**
1. Builds one embed titled `About Trophy Bot :trophy:` (`about.js:13`).
2. Description contains links to GitHub (`github.com/Aidanete/trophy-bot`), Ko-fi (`ko-fi.com/antikore`), the support server (`discord.gg/kNmgU44xgU`), and credits `@Antikore#9357` (`about.js:14-20`).
3. Thumbnail is the bot's avatar; color is `color.main` `#0096FF` (`about.js:22-23`, `globals.js:15-19`).

**Data operations:** none.

**Edge cases, quirks and bugs:** none in the command itself.

**Discrepancies with prior docs:** none material.

**Rust target:** Keep as a static embed. Update links/credits as needed; consider pulling the version string from `Cargo.toml`.

## /forgetme

**Purpose:** Owner-only, two-step deletion of all guild data and images, after which the bot leaves the guild.

**Definition** (`commands/bot/forgetme.js:5-8`)
- Parameters: none. Default permissions: Administrator (`setDefaultMemberPermissions("8")`). Cooldown: none. Public reply.

**Current behavior (validated)**
1. If the invoker is not the **guild owner**, the deferred reply is deleted and nothing else happens — the Administrator default permission is not sufficient (`forgetme.js:11-14`).
2. Otherwise shows a warning embed (error color `#E02D44`) explaining the deletion is irreversible (`forgetme.js:16-26`).
3. Attaches one danger-style button, custom id `forgetmeproceed`, label "Delete all server data", emoji 🧹 (`forgetme.js:36-46`). There is **no cancel button**.
4. Button handling lives in `events/button.js`: every button press is deferred ephemerally (`button.js:10`); only the guild owner's press of `forgetmeproceed` proceeds (`button.js:12-13`).
5. On proceed: ephemeral goodbye message (`button.js:14-16`), then `forgetMe(client, guild)` (`button.js:18`, `globals.js:367-385`):
   - Reads `data.${guild.id}.trophies`; for every trophy key except `current`, deletes the image file `./images/${image}` if set (`globals.js:369-378`).
   - Sets `data.${guild.id}` to `-1` — a **tombstone**, not a key removal (`globals.js:380`). `getServer` treats `-1` as "no data" and recreates fresh data if the bot is re-invited (`globals.js:416-419`).
   - Calls `guild.leave()` (`globals.js:381`). All errors are swallowed with a `console.error` (`globals.js:382-384`).

**Data operations**
- Read: `data.${guild}.trophies` (to find image filenames).
- Write: `data.${guild}` = `-1` (entire guild subtree replaced by tombstone).
- Filesystem: unlink `./images/{image}` per trophy image.

**Edge cases, quirks and bugs**
- **QUIRK:** Non-owner invocation silently deletes the reply (`forgetme.js:12`, not even awaited); the user gets no feedback.
- **QUIRK:** `events/button.js:20-25` handles a `forgetmenope` cancel button that is never rendered anywhere — dead code; the confirmation flow has no cancel option.
- **QUIRK:** Any other button press (or a non-owner press) falls through to an ephemeral "Not replied" message (`button.js:28-32`).
- **BUG (harmless):** `globals.js:376` does `await fs.unlink(path, callback)` — callback-style `fs.unlink` returns `undefined`, so the `await` is a no-op and unlink errors are ignored via the empty callback. Files may silently fail to delete.
- **QUIRK:** `forgetme.js:2` imports `sleep` and never uses it; `buttons(disabled)` (`forgetme.js:36`) is only ever called with the default.
- **QUIRK:** Data is not actually removed from the DB; the guild key remains with value `-1`.

**Discrepancies with prior docs**
- DISCORD_COMMANDS_DOCUMENTATION.md and CLAUDE.md describe "complete data deletion"; the real implementation writes a `-1` tombstone instead of deleting the key.
- Neither doc mentions the missing cancel button, the dead `forgetmenope` handler, or that image deletion errors are silently swallowed.

**Rust target:** Keep owner-only gate and explicit button confirmation, but add a real Cancel button and give non-owners an ephemeral rejection message. Perform a true delete: cascade-delete guild rows (guilds → trophies → user_trophies → settings → rewards → panels via FK CASCADE) inside a transaction, delete image files with logged errors (via `log`), then leave the guild. No tombstones.

## /help

**Purpose:** Static usage guide embed.

**Definition** (`commands/bot/help.js:5-7`)
- Parameters: none. Default permissions: none. Cooldown: none. Public reply.

**Current behavior (validated)**
1. Single embed, title `How to trophies 101`, main color (`help.js:13-14`).
2. Hardcoded description listing `/create`, `/award`, `/permissions (add|remove)`, `/delete`, `/edit`, `/revoke`, `/trophies`, `/leaderboard`, `/settings`, `/rewards` (`help.js:16-26`).

**Data operations:** none.

**Edge cases, quirks and bugs**
- **QUIRK:** The help text is outdated — it instructs users to grant `manage trophies` / `manage users` via `/permissions (add|remove) <permission> <role>` (`help.js:16`), i.e. the **deprecated** custom permission system that `imsafeWarning` itself tells users to stop using.

**Discrepancies with prior docs**
- DISCORD_COMMANDS_DOCUMENTATION.md says it "Explains permission system"; it actually explains the deprecated one, contradicting the bot's own migration messaging.

**Rust target:** Rewrite content around Discord-native permissions only (the custom permission system is not reimplemented). Consider generating the command list from the Poise framework registry instead of hardcoding.

## /imsafe

**Purpose:** One-way per-guild flag confirming the guild has configured Discord-native command permissions, unlocking legacy management commands.

**Definition** (`commands/bot/imsafe.js:5-8`)
- Parameters: none. Default permissions: Manage Guild (`setDefaultMemberPermissions("32")`). Cooldown: none. Public reply.

**Current behavior (validated)**
1. Reads `data.${guild}.imsafe`, defaulting to `false` (`imsafe.js:14`).
2. If already true: replies ":white_check_mark: You're currently on safe mode :)" (`imsafe.js:18-25`).
3. Otherwise sets the flag to `true` and confirms (`imsafe.js:27-34`). There is no way to unset it.

**What the flag gates:** in `events/command.js:39-44`, any command module declaring a `permissions` property (the legacy custom-permission commands) is blocked with `imsafeWarning` while `imsafe` is false, unless the caller is the hardcoded dev. Per the warning text (`globals.js:26`) the gated commands are `/award`, `/revoke`, `/clear`, `/create`, `/delete`, `/edit`, `/panel`, `/rewards`.

**Data operations**
- Read: `data.${guild}.imsafe`.
- Write: `data.${guild}.imsafe = true`.

**Edge cases, quirks and bugs**
- **QUIRK:** `getServer` creates new guilds with `imsafe: true` (`globals.js:437`), so the flag only ever blocks guilds whose data predates v1.4. The command is a legacy migration ratchet, not an ongoing safety feature.
- **QUIRK:** One-way: no command can set the flag back to false.

**Discrepancies with prior docs**
- CLAUDE.md ("'imsafe' mode required for management commands") and DISCORD_COMMANDS_DOCUMENTATION.md omit that new guilds default to safe, and that the dev ID bypasses the check.

**Rust target:** Do not reimplement the gate — the custom permission system is gone and Discord-native permissions are the only mechanism, so the flag becomes unnecessary. Keep the `is_safe` column only if needed during data migration for historical record; `/imsafe` can be dropped or turned into a no-op informational reply.

## /invite

**Purpose:** Shows the bot's OAuth2 invite link.

**Definition** (`commands/bot/invite.js:5-7`)
- Parameters: none. Default permissions: none. Cooldown: none. Intended ephemeral, **actually public** (see bug).

**Current behavior (validated)**
1. Embed with title "Invite Me to Your Server!", bot avatar thumbnail, main color (`invite.js:13-16`).
2. Hardcoded invite URL with `client_id=985134052665356299&permissions=34816&scope=applications.commands%20bot` (`invite.js:15`).

**Data operations:** none.

**Edge cases, quirks and bugs**
- **BUG:** `ephemeral: true` on `editReply` (`invite.js:20`) has no effect because the dispatcher already deferred publicly (`events/command.js:14`). The reply is public.
- **QUIRK:** Client ID and permission bits are hardcoded rather than derived from the running client.

**Discrepancies with prior docs**
- DISCORD_COMMANDS_DOCUMENTATION.md claims "Ephemeral: true"; the reply is actually public.

**Rust target:** Build the URL from the running application ID; reply genuinely ephemerally (Poise lets each command control defer/ephemeral).

## /language (dead)

**Purpose:** Former language selector. **Fully dead.**

**Current behavior (validated)**
- The entire `module.exports` block is commented out (`commands/bot/language.js:5-43`); only the top `require`s execute (`language.js:1-3`). The module exports `{}`, and `fetchModules` skips modules without `data.name` (`globals.js:310`), so the command is never registered.

**Data operations:** none (dead code).

**Discrepancies with prior docs:** none — both docs correctly mark it dead.

**Rust target:** Do not implement. The language/localization system is out of scope for the rewrite.

## /ping

**Purpose:** Shows bot latency and Discord WebSocket ping.

**Definition** (`commands/bot/ping.js:5-7`)
- Parameters: none. Default permissions: none. Cooldown: none. Public reply.

**Current behavior (validated)**
1. Reads `interaction.client.ws.ping`, rounded, as "Discord API" (`ping.js:14`).
2. Edits the deferred reply to `Pinging...` with `fetchReply: true` (`ping.js:15`).
3. "Bot Latency" = `sent.createdTimestamp - interaction.createdTimestamp` (`ping.js:19`).
4. Final edit: content `Done!` plus the embed (`ping.js:24-27`).

**Data operations:** none.

**Edge cases, quirks and bugs**
- **QUIRK:** Because the reply was already created by the dispatcher's `deferReply`, `sent.createdTimestamp` is the timestamp of the **deferral message**, not of this edit — "Bot Latency" measures interaction-to-defer time, not a fresh round trip.

**Discrepancies with prior docs**
- DISCORD_COMMANDS_DOCUMENTATION.md describes latency as "response time - interaction time" without noting it is really the defer timestamp.

**Rust target:** Report shard gateway latency (Serenity shard runner latency) and a measured round-trip (timestamp before/after an initial response edit). Keep the two-metric embed format.

## /stats

**Purpose:** Shows global bot statistics (Discord footprint + lifetime counters).

**Definition** (`commands/bot/stats.js:5-8`)
- Parameters: none. Default permissions: none. Cooldown: declared `cooldown: 10` (`stats.js:5`) but **never enforced** (see dispatcher quirks). Public reply.

**Current behavior (validated)**
1. Field "Discord": `client.guilds.cache.size` servers, `client.users.cache.size` users, `timeFormat(client.uptime)` uptime (`stats.js:24-26`, `globals.js:271-281`).
2. Field "Trophies": `client.commands.size` loaded commands, plus three quick.db counters with `?? 0` fallbacks (`stats.js:34-37`).

**Data operations (reads only)**
- `data.commands.total` — cumulative lifetime command runs (incremented by the dispatcher on every command, `events/command.js:48-49`).
- `data.trophies` — cumulative trophies-created counter (incremented by `/create`, decremented by `/delete`).
- `data.trophiesAwarded` — cumulative awards counter.

**Edge cases, quirks and bugs**
- **BUG:** The 10-second cooldown is dead metadata; nothing reads `module.cooldown`.
- **QUIRK:** "Users" is only the **user cache size**, whose accuracy depends on the daily `AttemptToFetchUsers` sweep (`globals.js:482-496`); it is not a real unique-user count.
- **QUIRK:** All three DB counters are lifetime-cumulative and known to be inflated relative to current live data (e.g. `data.trophies` counts trophies from guilds that later left/were forgotten; `data.commands.total` counts failed runs too). Treat them as vanity metrics, not as reconcilable totals.
- **QUIRK:** "Commands" is the number of loaded command modules, not a stat from the DB.

**Discrepancies with prior docs**
- DISCORD_COMMANDS_DOCUMENTATION.md and CLAUDE.md state a working 10-second cooldown; it is never enforced.
- Neither doc notes that the user count is cache-based or that the counters are cumulative/inflated.

**Rust target:** Read counters from the `bot_stats` table; migrate legacy counters as-is but document them as historical/cumulative. Increment command counters only on successful execution (or track success/failure separately). Implement a real cooldown in the framework (Poise has built-in cooldown support). Compute server count from cache; consider dropping or clearly labeling the user count.

## /suggest

**Purpose:** Static redirect to the support server (the old suggestion system was removed in 1.4).

**Definition** (`commands/bot/suggest.js:5-8`)
- Parameters: none. Default permissions: none. Cooldown: declared `cooldown: 10` (`suggest.js:5`) but never enforced. Public reply.

**Current behavior (validated)**
1. Single embed, title ":people_hugging: Migrating Suggestions", linking `discord.gg/kNmgU44xgU` (`suggest.js:14-16`).

**Data operations:** none. (A `client.suggestionChannel` is fetched at `events/ready.js:13` but never used by this command — leftover from the removed suggestion system.)

**Edge cases, quirks and bugs**
- **BUG:** Declared cooldown is dead metadata (dispatcher never enforces it).

**Discrepancies with prior docs**
- DISCORD_COMMANDS_DOCUMENTATION.md claims a working 10-second cooldown.

**Rust target:** Keep as a static redirect, or merge into `/support` — two static commands pointing at the same server is redundant.

## /support

**Purpose:** Static embed with support server, GitHub issues link, and a pointer to `/suggest`.

**Definition** (`commands/bot/support.js:5-7`)
- Parameters: none. Default permissions: none. Cooldown: none. Intended ephemeral, **actually public** (same bug as `/invite`).

**Current behavior (validated)**
1. Embed titled ":question: You need support?" with support server and `github.com/Aidanete/trophy-bot/issues` links (`support.js:15-20`).

**Data operations:** none.

**Edge cases, quirks and bugs**
- **BUG:** `ephemeral: true` on `editReply` (`support.js:25`) is ineffective after the public defer — the reply is public.
- **QUIRK:** `if (!interaction) return;` (`support.js:22`) is dead code — `interaction` can never be falsy there.

**Discrepancies with prior docs**
- DISCORD_COMMANDS_DOCUMENTATION.md claims "Ephemeral: true"; the reply is actually public.

**Rust target:** Keep static; make it genuinely ephemeral; drop the dead null check. Candidate for consolidation with `/suggest` and `/about`.
