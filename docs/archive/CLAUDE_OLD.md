# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## ⚠️ MIGRATION CONTEXT

**Production System:** Node.js v18 with Discord.js v14 (ACTIVE - 100% of traffic)
**Target System:** Rust with Serenity 0.12 + Poise 0.6 (IN DEVELOPMENT - 0% of traffic)
**Migration Status:** Planning phase - Rust skeleton exists but no commands implemented

**Critical:** This document prioritizes accurate documentation of the **Node.js production system** to enable a safe, zero-downtime migration to Rust. All information must reflect actual runtime behavior, not just code existence.

### Migration-Critical Summary (Node.js Production → Rust)

**What Actually Works in Production:**
- ✅ 24 active slash commands (NOT 25 or 26)
- ✅ 31 actively used functions from globals.js (36 exported, 5 never called)
- ✅ 5 actively used constants (color, emoji, settings, supportServer, testingServers)
- ✅ Bot DB: 5 actively used fields (commands, trophies, trophiesAwarded, lastDay, milestone)
- ✅ Guild DB: 7 actively used fields (trophies, users, settings, rewards, panel, imsafe, permissions)

**Dead Code (DO NOT Migrate to Rust):**
- ❌ `/language` command (fully commented out in source code)
- ❌ `/trophystats` command (0 bytes empty file)
- ❌ 5 exported but unused: booleans (constant), clearMentions(), isAlphanumeric(), isBanned(), showCooldown()
- ❌ Cooldown system (declared in stats.js/suggest.js but never enforced in events/command.js)
- ❌ Custom permissions (deprecated, only imsafe flag check remains in production)
- ❌ Language loading (events/ready.js loads locale files but /language command is disabled)

**Legacy Database Fields (exist in production data but never used in code):**
- ⚠️ `.version`, `.defaultLanguage`, `.bannedUsers` (bot DB - initialized once, never read)
- ⚠️ `.language` (guild DB - per-server setting for disabled language system)
- ⚠️ `.restapi` (guild DB - in 95.7% of guilds but NO implementation exists)

**Note for Rust Migration:** These legacy fields may need to be included in data migration tools for compatibility, but should NOT be implemented in Rust application logic.

## Project Overview

Trophy Bot is a **gamification and community recognition system** for Discord servers. It transforms subjective appreciation into a structured achievement system where:

- **Server admins** create custom trophies with names, descriptions, images, and point values
- **Members** receive trophies for contributions and see their progress on leaderboards
- **Communities** benefit from increased engagement through competition and role progression
- **Recognition** becomes visible, measurable, and rewarding for all participants

The bot solves the problem of informal appreciation by creating tangible rewards, social status, and competitive motivation within Discord communities.

**Production Statistics (from bot_db.json - VERIFIED PRODUCTION DATA):**
- Total commands executed: 104,913
- Total trophies created: 10,571
- Total trophies awarded: 120,411
- Total guilds: 2,493 (from guilds_db.json)
- Most used command: `/award` (41,240 uses - 39.3%)
- Least used command: `/export` (28 uses - 0.03%)

**Development Guidelines:**

For Rust code:
- Does not use println or eprintln instead use log crate
- Try to avoid to put code in main function, it must be modular using modules and functions
- Always after change code, run cargo test to ensure no errors

## Architecture

### Core Structure

- **Entry point**: `index.js` - Initializes Discord.js v14 client and loads events
- **Database**: Uses `quick.db` which creates SQLite with JSON storage:
  - Two SQLite "tables" (`bot`, `guilds`), each with single row containing giant JSON
  - `client.db.bot` - Global bot data accessed via path notation
  - `client.db.guilds` - All server data in single JSON blob accessed via paths
- **Event system**: Files in `/events/` directory handle Discord events
- **Command system**: Organized in `/commands/` by category:
  - `/commands/bot/` - Bot utility commands (help, ping, stats, etc.)
  - `/commands/manage/` - Trophy management commands (create, edit, award, etc.)
  - `/commands/users/` - User-facing commands (trophies, leaderboard, show)
- **Core logic**: `globals.js` contains all shared functions, settings, and utilities
- **Localization**: `/locale/languages/` contains language files

### Database Schema (ACTUAL WORKING STRUCTURE)

**IMPORTANT**: quick.db stores everything as JSON in SQLite with only 2 tables:
- `bot` table: Single row with JSON containing all global bot data
- `guilds` table: Single row with JSON containing ALL server data for ALL servers

All data accessed via path notation like `client.db.guilds.get('data.${guild}.trophies.${id}'):`

**Bot global data:**
- `client.db.bot.get('data.version')`
- `client.db.bot.get('data.commands.total')`
- `client.db.bot.get('data.trophies')` - Total trophies created
- `client.db.bot.get('data.trophiesAwarded')`

**Guild data (per server):**
- `client.db.guilds.get('data.${guild}.trophies.current')` - Next trophy ID counter
- `client.db.guilds.get('data.${guild}.trophies.${id}')` - Trophy definitions
- `client.db.guilds.get('data.${guild}.users.${user}.trophies')` - Array of trophy IDs
- `client.db.guilds.get('data.${guild}.users.${user}.trophyValue')` - Total score
- `client.db.guilds.get('data.${guild}.settings.${setting}')` - Server settings
- `client.db.guilds.get('data.${guild}.rewards')` - Array of role reward objects
- `client.db.guilds.get('data.${guild}.permissions.${permission}')` - Arrays of role IDs per permission
- `client.db.guilds.get('data.${guild}.panel')` - Leaderboard panel config {message, channel}
- `client.db.guilds.get('data.${guild}.imsafe')` - Safety mode flag
- `client.db.guilds.get('data.${guild}.language')` - Server language (deprecated)

### IMPORTANT: Non-functional Code (DO NOT MIGRATE TO RUST)

**Completely Inactive Files:**
- `/commands/bot/language.js` - **ENTIRE module.exports commented out (lines 5-43)**
- `/commands/users/trophystats.js` - **Empty file (0 bytes)**
- `/commons/database.js` - Mongoose functions (mongoose not used)
- `/commons/models.js` - Mongoose models (not used)
- `/commons/schemas.js` - Mongoose schemas (not used)
- `/commons/configs.js` - Empty stub, real config in globals.js
- `/commons/locale.js` - Empty stub, locale loading in globals.js
- `/commons/utils.js` - Empty file
- `/TrophyBot-Copy/` - Complete backup directory (ignore)
- `/windows/` - UI classes (deprecated, empty)

**Inactive Features:**
- **Language system**: Loaded in `events/ready.js:19` but `/language` command completely commented out
- **Custom permissions system**: Deprecated (CLAUDE.md incorrectly states it works - only validation check remains)
- **Cooldown system**: Properties exist (`stats.js`, `suggest.js`) but **NOT enforced** in `events/command.js`
- **REST API**: Field `.restapi` initialized in `getServer()` but no implementation exists

## Development Commands

### Local Development
- `npm start` - Start the bot directly with Node.js
- `node index.js` - Alternative way to start the bot

### Docker Development (via dev.sh)
- `./dev.sh build` - Build Docker container
- `./dev.sh run` - Start container in foreground
- `./dev.sh up` - Start container in background
- `./dev.sh down` - Stop container
- `./dev.sh logs` - View container logs
- `./dev.sh clean` - Remove container and images
- `./dev.sh` (no args) - Full rebuild and run cycle

## Environment Setup

Required environment variables (see `.env.example`):
- `DISCORD_TOKEN` - Discord bot token
- `DISCORD_BOT_ID` - Bot's client ID
- `DEBUG` - Enable debug mode (true/false)

## Core Bot Behavior (For Rust Rewrite)

### Command Flow & Business Logic

**Trophy Creation (`/create`):**
1. Validate inputs (name ≤32 chars, description ≤128 chars, value between -999999 and 999999)
2. Check guild trophy limit (150 trophies max per guild)
3. Generate next trophy ID: `current + 1`
4. Handle image attachment (PNG/JPG/JPEG/GIF, max 1MB, save as `{guild}_{id}.{ext}`)
5. Parse dedication (can be user ID, mention, or plain text)
6. Store trophy object with: creator, created timestamp, name, description, emoji, value, image, dedication, details, signed flag
7. Increment global trophy counter

**Trophy Awarding (`/award`):**
1. Check if trophy exists in guild
2. Add trophy ID to user's trophy array
3. Add trophy value to user's total score
4. Check role rewards and assign if threshold reached
5. Increment global trophies awarded counter

**Leaderboard System:**
1. Sort users by total trophy value (descending)
2. Apply settings: hide quit users, leaderboard display format
3. Paginate results (configurable items per page)
4. Support for permanent panels that auto-update

**Permission System:**
1. Commands have built-in Discord permissions (manage server, etc.)
2. Custom role-based permissions stored per guild
3. "imsafe" mode required for management commands (safety mechanism)
4. Bypass permissions for bot developers (hardcoded user IDs)

### Data Models

**Trophy Object:**
```json
{
  "creator": "user_id",
  "created": timestamp,
  "name": "string(32)",
  "description": "string(128)", 
  "emoji": "string(64)",
  "value": "number(-999999 to 999999)",
  "image": "filename or null",
  "dedication": {"user": "id or null", "name": "string(32)"},
  "details": "string(300)",
  "signed": boolean
}
```

**User Data:**
```json
{
  "trophies": ["trophy_id_array"],
  "trophyValue": "total_score_number"
}
```

**Guild Settings:**
- `dedication_display`: 0=Always Mention, 1=Always Name, 2=Mention Only in Server
- `stack_roles`: 0=Stack All, 1=Only Highest  
- `hide_unused_trophies`: 0=Hide, 1=Show
- `hide_quit_users`: 0=Hide, 1=Show
- `leaderboard_format`: 0=Mention, 1=Username, 2=Nickname, 3=Username+Tag

### Event Handlers Needed

1. **Ready Event**: Load commands, set activity status, initialize database
2. **InteractionCreate**: Handle slash commands, validate permissions, error logging
3. **GuildCreate/Delete**: Track bot joins/leaves
4. **Button Interactions**: For paginated embeds
5. **User Updates**: For role reward automation

### Essential Functions from globals.js (PRODUCTION VERIFIED)

**Location:** `globals.js` (734 lines, 37 functions defined, 42 total exports)
**Total Exported:** 36 functions + 6 constants = **42 total exports**
**Actually Used in Production:** 31 functions + 6 constants (see usage analysis below)
**Internal-Only (not exported):** 1 function (getRandomActivity)

**Core Utilities (Used in Commands):**
- `getPage(array, perPage, currentPage)` → Pagination logic, returns `{list, page, last}` (leaderboard, rewards, trophies)
- `parseUser(client, input, fallback, guild, fetchFromDiscord)` → Parse user mentions/IDs/names (create, edit, trophies)
- `downloadImage(url, path)` → Save trophy images using node-fetch (create, edit)
- `getSetting(client, guild, setting)` → Get guild configuration with defaults (details, show, leaderboard, trophies)
- `getTrophyCount(client, guild)` → Count guild's trophies excluding 'current' key (create.js:24)
- `getTrophy(client, guild, trophy)` → **Fuzzy search** trophy by ID or name (award, revoke, delete, edit, details, show)
- `getServer(client, guildId, guild)` → Initialize/get guild data structure (command.js:21)

**Display & Formatting (User-Facing):**
- `showError(message)` → Format: `${emoji.error} **Oopsie!** ${message}` (create, panel)
- `showSuccess(message)` → Format: `${emoji.success} **Great!** ${message}` (panel)
- `getMedal(i)` → Returns 🥇🥈🥉 for positions 1-3, else `:medal:` (leaderboard)
- `getDedication(guild, dedication, config)` → Format trophy dedication based on settings (details, show)
- `parseFormat(config, guild, id, pre)` → User display format for leaderboards (leaderboard.js:641)
- `timeFormat(ms)` → Uptime formatting: "5h 23m 12s" (stats.js:26)

**Permission & Validation:**
- `imsafeWarning(interaction)` → Safety mode warning embed (command.js:42)
- `isDev(userId)` → Check hardcoded dev ID: `'353998390734094346'` (command.js:39)
- `isInServer(guild, user)` → Check `guild.members.cache.get(user) != undefined` (leaderboard.js:626)
- `anyIn(array1, array2)` → Array intersection: `array1.some(x => array2.includes(x))` (command.js:39, trophies.js:110)

**Database Operations:**
- `cleanseTrophies(client, guild, trophy, value)` → Remove trophy from all users (delete.js:50)
- `parseName(text)` → Normalize for fuzzy search: `toLowerCase().replace(/\W/g, '').replace(/ /g, '')` (settings.js:60)
- `checkName(first, second)` → Substring match: `second.includes(first)` (settings.js:64)

**Background Tasks (events/ready.js):**
- `fetchModules(directory, extension, recursive)` → Load commands/languages, auto-register slash commands (ready.js:18-19)
  - **Critical behavior:** Filters files by `module?.data?.name` existence (globals.js:310)
  - Skips any file where `module.exports.data.name` is undefined or missing
  - This is why `/language` (commented exports) and `/trophystats` (empty file) don't register
  - Returns Collection with command name as key, full module as value
- `changeActivity(client)` → **Infinite loop**, 60s status rotation, 8 random messages (ready.js:34)
  - Uses getRandomActivity() internal function to generate status text
  - Activities: Watching guilds, Listening to trophies, Playing with users, etc.
- `updatePanels(client)` → **Infinite loop**, 60s interval, updates leaderboard panels (ready.js:36)
  - Iterates through ALL guilds every 60s checking for panel config
  - 1s delay between guild updates to prevent rate limiting (globals.js:693)
- `updatePanel(client, guild)` → Single panel update (panel.js:37)
  - Edits existing message with fresh leaderboard data
- `AttemptToFetchUsers(client, force)` → **Daily** member cache warming, prevents "quit user" false positives (ready.js:38, command.js:24)
  - Uses lastDay field to track if already run today
  - Force-fetches all members in all guilds to warm Discord.js cache
  - Critical for leaderboard "hide quit users" accuracy
- `fixShit(client)` → Production hotfixes/migrations (mostly commented, emergency tool) (ready.js:25)
  - Used for one-off data fixes, most code commented out
  - Contains historical migration logic (kept for reference)

**User Interaction:**
- `forgetMe(client, guild)` → Complete data deletion + bot leave server (button.js:18)
- `attemptFetchIfCacheCleared(keys, guild)` → Smart cache refresh if `keys.length > cache.size` (leaderboard.js:623)
- `sleep(ms)` → Promise-based delay (forgetme.js:24)

**Constants (Exported and Used):**
- `color` → `{main: "#0096FF", error: "#E02D44", success: "#32CD32"}` (used in ALL commands)
- `emoji` → `{trophy: "🏆", success: "✅", error: "<:error:985469320844967946>"}` (used in 13 commands)
- `settings` → Array of 5 setting configurations (used in settings.js:17)
- `supportServer` → `'985439832388042822'` (used in user.js:12, events/join.js)
- `testingServers` → `['985439832388042822', '1393760778972041258']` (used ONLY in fetchModules for DEBUG mode)

---

**Exported But NEVER Used (DO NOT IMPLEMENT IN RUST):**
- ❌ `booleans` → `{true: ['yes'...], false: ['no'...]}` constant - Exported but never imported anywhere
- ❌ `clearMentions(message)` → Sanitizes @everyone/@here - Exported but never called
- ❌ `isAlphanumeric(str)` → `/^[0-9A-Za-z]+$/` check - Exported but never called
- ❌ `isBanned(id)` → Always returns `false` - No ban system exists, dead code
- ❌ `showCooldown(time)` → Cooldown message - Cooldown system NOT implemented

### Complete Command Reference (PRODUCTION VERIFIED)

**📋 Detailed Implementation:** See @DISCORD_COMMANDS_DOCUMENTATION.md for comprehensive specifications

**Active Commands: 24 (2 completely disabled in production)**

**IMPORTANT:** Although 26 commands are listed in bot_db.json stats, only **24 actually register and execute** in production. The `/language` and `/trophystats` commands are completely non-functional (one commented out, one empty file).

#### Bot Utility Commands (9 active, 1 disabled)
- ✅ `/about` - Bot information and links
- ✅ `/forgetme` - Complete data deletion + bot leave server (owner only)
- ✅ `/help` - Command usage guide
- ✅ `/imsafe` - Enable management commands (required once per server)
- ✅ `/invite` - Bot invite link
- ❌ `/language` - **DISABLED** (commands/bot/language.js lines 5-43 fully commented out)
- ✅ `/ping` - Latency check
- ✅ `/stats` - Bot statistics (has `cooldown: 10` but NOT enforced)
- ✅ `/suggest` - Redirects to support server (has `cooldown: 10` but NOT enforced)
- ✅ `/support` - Support server link

#### Trophy Management Commands (12 active)
- ✅ `/award` - Award trophies to users (1-50 count, fuzzy trophy search)
- ✅ `/clear` - Reset user's trophies and score to 0
- ✅ `/create` - Create new trophy (max 150 per guild, image support)
- ✅ `/delete` - Delete trophy + auto-remove from all users
- ✅ `/details` - Show private trophy details (manage permission)
- ✅ `/edit` - Edit existing trophy (all fields modifiable)
- ✅ `/revoke` - Remove trophies from users (1-50 count)
- ✅ `/export` - Export guild data as JSON (admin only)
- ✅ `/panel` - Create/delete auto-updating leaderboard panel
- ✅ `/perms` - **DEPRECATED** Shows migration notice only
- ✅ `/rewards` - Manage role rewards (max 20 per guild)
- ✅ `/settings` - Configure 5 guild settings

#### User-Facing Commands (3 active, 1 disabled)
- ✅ `/leaderboard` - Server rankings (10 per page, medals for top 3)
- ✅ `/show` - Display trophy information (public view)
- ✅ `/trophies` - View user/guild trophy lists (paginated)
- ❌ `/trophystats` - **DISABLED** (commands/users/trophystats.js is 0 bytes empty file)

---

**Cooldown System Status:** ⚠️ **NOT IMPLEMENTED**
- `commands/bot/stats.js:5` and `suggest.js:5` declare `cooldown: 10`
- `events/ready.js:28` initializes `client.cooldowns = new Collection()`
- `events/command.js` **NEVER checks or enforces** cooldowns
- **Conclusion:** Cooldown properties are dead code with no runtime effect

**Custom Permissions Status:** ⚠️ **DEPRECATED**
- Commands have `permissions: ['manage_users']` arrays
- Only used in `events/command.js:39` to check `imsafe` flag
- Real permissions: Discord native (setDefaultMemberPermissions)
- `/perms` command shows deprecation notice

**Language System Status:** ⚠️ **DISABLED**
- `events/ready.js:19` loads languages from `/locale/languages/`
- Stored in `client.languages` Collection
- `/language` command completely commented out
- No command uses language system

### Command Implementation Details

**Critical Command Behaviors:**

**`/create` (Complex Implementation):**
- Parameters: `name` (required), `description`, `emoji`, `value`, `dedication`, `signed`, `image`, `details`
- Validation: name ≤32 chars, description ≤128 chars, emoji ≤64 chars, value -999,999 to 999,999
- Image handling: PNG/JPG/JPEG/GIF, max 1MB, saved as `{guild_id}_{trophy_id}.{extension}`
- ID generation: Auto-increment from `trophies.current` counter
- Dedication parsing: Can be user ID, mention (@user), or plain text

**`/award` & `/revoke` (Bulk Operations):**
- Parameters: `trophy` (name or ID), `user`, optional `count` (1-50)
- Database: Add/remove entries in user_trophies, update score calculations
- Role rewards: Trigger automatic role assignment/removal based on new scores
- Validation: Trophy must exist, count within limits

**`/settings` (5 Configuration Options):**
1. **dedication_display**: Always Mention (0), Always Name (1), Mention Only in Server (2)
2. **stack_roles**: Stack All (0), Only Highest Reward (1)
3. **hide_unused_trophies**: Hide (0), Show (1) - affects non-managers viewing guild trophies
4. **hide_quit_users**: Hide (0), Show (1) - affects leaderboard display
5. **leaderboard_format**: Mention (0), Username (1), Nickname (2), Username+Tag (3)

**`/rewards` (Role Management System):**
- Max 20 role rewards per server
- Minimum requirement: 1 point
- Automatic assignment when user scores reach thresholds
- Respects Discord role hierarchy (bot can't assign roles above its position)

**`/panel` (Persistent Leaderboard):**
- Creates/deletes auto-updating message in channel
- Only one panel per server allowed
- Stored as `{message_id, channel_id}` in guild data
- Updates automatically when scores change

**Special Behaviors:**
- **`/permissions`**: Shows deprecation message only, redirects to Discord native permissions
- **`/export`**: Admin-only, generates JSON file with all server data
- **`/forgetme`**: Server owner only, complete data deletion + bot leaves server
- **`/stats`**: 10-second cooldown, shows global bot statistics
- **`/suggest`**: 10-second cooldown, redirects to support server

**Pagination System:**
- Leaderboards: 10 users per page, medals (🥇🥈🥉) for top 3
- Trophy lists: 10 trophies per page
- Navigation: Page numbers in footer, validated page ranges

**Error Handling:**
- User-friendly messages with support server links
- Automatic error logging to developer channels
- Graceful handling of missing permissions, invalid inputs
- Clear validation feedback for limits exceeded

### Validation Rules & Constraints

**Trophy Creation Limits:**
- Name: 1-32 characters
- Description: 1-128 characters  
- Emoji: 1-64 characters
- Value: -999,999 to 999,999 points
- Dedication: 1-32 characters
- Details: 1-300 characters
- Images: PNG/JPG/JPEG/GIF, max 1MB
- Max 150 trophies per server

**Command Limits:**
- Award/Revoke: 1-50 count per command
- Role Rewards: Max 20 per server, minimum 1 point requirement
- Leaderboard: 10 users per page, medals for positions 1-3
- Trophy Lists: 10 trophies per page

**Permission Requirements:**
- Management commands require `/imsafe` to be run first
- Default Discord permissions: Manage Guild (32) or Administrator (8)
- Bot developers bypass all restrictions (hardcoded user IDs)
- Role hierarchy respected for role rewards

### File Storage
- Trophy images: `./images/{guild_id}_{trophy_id}.{extension}`
- Database: SQLite file (`json.sqlite`) with 2 tables, each containing single JSON blob
- Supported image formats: PNG, JPG, JPEG, GIF (max 1MB)

### Critical Architecture Note for Rust Rewrite

**quick.db Reality**: This is NOT a normalized database. It's essentially:
```sql
-- Table: bot
| id | json |
|----|------|  
| data | {"version": 0, "commands": {...}, "trophies": 123, ...} |

-- Table: guilds  
| id | json |
|----|------|
| data | {"guild1": {"trophies": {...}}, "guild2": {...}, ...} |
```

**Implications for Rust Rewrite**:
- Current system loads ENTIRE guild JSON for any operation
- No indexing, no joins, no SQL queries - pure JSON manipulation
- All server data lives in single JSON blob (major scalability bottleneck)
- **MUST implement proper normalized database design for production scale**

### Production Hardcoded Values (MUST BE CONFIGURABLE IN RUST)

**Channel IDs (events/ready.js):**
- Error Channel: `985869722199416862` (line 12) - Receives error logs from all servers
- Suggestion Channel: `985872094153830400` (line 13) - Currently unused

**Server IDs:**
- Support Server: `985439832388042822` (globals.js:397, events/user.js:12)
- Testing Servers: `['985439832388042822', '1393760778972041258']` (globals.js:399-402)
  - Used only in `fetchModules` when `DEBUG=true`

**Role IDs:**
- Support Server Welcome Role: `985440033286787123` (events/user.js:16)

**User IDs:**
- Bot Developer: `353998390734094346` (globals.js:389, isDev function)
  - Bypasses all permission checks
  - Receives milestone notifications

**Custom Emoji:**
- Error Emoji: `<:error:985469320844967946>` (globals.js:108)
  - If emoji deleted/bot leaves source server, will show as `:error:` text

**Timings (Background Tasks):**
- Activity rotation: 60,000ms (globals.js:458 - changeActivity loop)
- Panel updates: 60,000ms (globals.js:686 - updatePanels loop)
- Initial activity delay: 10,000ms (globals.js:450 - "Starting up!" message)
- Per-guild delay in panel updates: 1,000ms (globals.js:693)

**For Rust Migration:**
- All IDs → Environment variables or config file
- Timings → Configurable constants
- Custom emoji → Fallback to Unicode emoji if custom unavailable

---

### Database Fields Usage (PRODUCTION ANALYSIS)

**Bot Database (`client.db.bot.data`):**
```javascript
// ACTIVELY USED IN PRODUCTION:
.commands.total           // ✅ Incremented: events/command.js:49, shown in /stats
.commands.{commandName}   // ✅ Incremented: events/command.js:48, tracked per command (26 tracked, 24 active)
.trophies                 // ✅ Incremented: commands/manage/create.js, shown in /stats
.trophiesAwarded          // ✅ Incremented: commands/manage/award.js, shown in /stats
.lastDay                  // ✅ Used: globals.js:485 (AttemptToFetchUsers daily tracking, tracks day number for cache warming)
.milestone                // ✅ Used: events/join.js:18 (one-time 75 servers notification flag)

// INITIALIZED BUT NEVER READ (DO NOT MIGRATE):
.version                  // ❌ Set to 0 in ready.js:42, never accessed anywhere
.defaultLanguage          // ❌ Set to 'en', language system completely disabled
.bannedUsers              // ❌ Empty array, isBanned() always returns false, no ban system
```

**Guilds Database (`client.db.guilds.data.${guild}`):**
```javascript
// ACTIVELY USED:
.trophies.current         // ✅ Auto-increment counter for trophy IDs
.trophies.{id}            // ✅ Trophy definitions (all fields used)
  ├─ .creator             // ✅ User ID who created trophy
  ├─ .created             // ✅ Unix timestamp
  ├─ .name                // ✅ Trophy name (max 32 chars)
  ├─ .description         // ✅ Trophy description (max 128 chars)
  ├─ .emoji               // ✅ Trophy emoji (max 64 chars)
  ├─ .value               // ✅ Point value (-999999 to 999999)
  ├─ .image               // ✅ Filename: "{guild}_{id}.{ext}" or null
  ├─ .dedication          // ✅ {user: "id or null", name: "name or null"}
  ├─ .details             // ✅ Private details (max 300 chars)
  └─ .signed              // ✅ Boolean, shows creator in display

.users.{userId}.trophies      // ✅ Array of trophy IDs (duplicates allowed)
.users.{userId}.trophyValue   // ✅ Total score (sum of trophy values)

.settings.dedication_display  // ✅ 0-2, how to show trophy dedications
.settings.stack_roles         // ✅ 0-1, role reward behavior
.settings.hide_unused_trophies // ✅ 0-1, visibility for non-managers
.settings.hide_quit_users     // ✅ 0-1, leaderboard ex-member display
.settings.leaderboard_format  // ✅ 0-3, user display format

.rewards                  // ✅ Array: [{role: "id", requirement: number}]
.panel                    // ✅ Object: {message: "id", channel: "id"}
.imsafe                   // ✅ Boolean flag for management command access
.permissions              // ✅ Used in /trophies guild for hide_unused_trophies feature
  ├─ .manage_trophies     // ✅ Read in trophies.js:103 to check if user can see unused trophies
  ├─ .manage_users        // ⚠️ Declared in command metadata but only imsafe enforced
  └─ .manage_rewards      // ⚠️ Declared in command metadata but only imsafe enforced

// INITIALIZED BUT UNUSED (LEGACY/ABANDONED FEATURES):
.language                 // ❌ Language system disabled, /language command fully commented out
.restapi                  // ❌ {token: '', enabled: false} - Present in 95.7% (2,387/2,493) guilds but NO implementation exists
                          //    This was a planned feature that never launched, initialized in getServer() but never used
```

**For Rust Migration:**
- ✅ **Implement:** All 7 actively used guild fields (trophies, users, settings, rewards, panel, imsafe, permissions.manage_trophies)
- ✅ **Implement:** All 5 actively used bot fields (commands, trophies, trophiesAwarded, lastDay, milestone)
- ❌ **Skip entirely:** `.version`, `.defaultLanguage`, `.bannedUsers` (bot DB - initialized but never read)
- ❌ **Skip entirely:** `.language` (guild DB - language system completely disabled)
- ⚠️ **Include for data migration only:** `.restapi` (guild DB - in 95.7% of guilds, needed to preserve existing data)

---

### Recommended Rust Architecture (ACTUAL IMPLEMENTATION)

**Technology Stack (CORRECTED):**
- **Discord API**: [Serenity](https://github.com/serenity-rs/serenity) v0.12 + [Poise](https://github.com/serenity-rs/poise) v0.6
  - ⚠️ CLAUDE.md previously said Twilight (incorrect)
  - Current Cargo.toml uses Serenity + Poise
- **Database**: [SQLx](https://github.com/launchbadge/sqlx) - Compile-time checked SQL (planned, not yet implemented)
- **Edition**: 2024 (Rust 2024 Edition, valid as of October 2024)

**Key Advantages Over Node.js Version:**
- **Compile-time SQL validation** - Catch database errors at build time
- **True async/await** with Rust's superior async runtime (Tokio)
- **Memory safety** - No garbage collection, predictable resource usage
- **Performance** - 10-100x faster than Node.js for database operations
- **Scalability** - Handle thousands of concurrent Discord interactions

**Current Rust Status:**
- ✅ Basic bot structure (src/bot/mod.rs)
- ✅ Serenity + Poise framework setup
- ✅ Migration schema ready (migrations/001_initial_schema.sql)
- ✅ Sharding support configured
- ✅ Rust 2024 Edition configured (valid as of October 2024)
- ⚠️ Only demo command (`/age`) implemented for testing
- ❌ No database connection yet
- ❌ No production commands (0/24 implemented)

**Migration Priority:**
1. Database connection (PostgreSQL with SQLx)
2. Core data models (Trophy, User, Guild structures)
3. Essential commands: `/create`, `/award`, `/trophies`, `/leaderboard`, `/show`
4. Management commands: `/delete`, `/edit`, `/revoke`, `/clear`
5. Configuration: `/settings`, `/rewards`, `/panel`
6. Utility: `/help`, `/about`, `/stats`, `/ping`
7. Skip completely: `/language`, `/trophystats` (dead code)

## Normalized Database Schema for Rust

**Required Tables:** 7 main tables
1. guilds (id, name, is_safe, created_at)
2. trophies (id, guild_id, creator_user_id, name, description, emoji, value, image_filename, dedication_user_id, dedication_text, details, signed, created_at)
3. user_trophies (id, guild_id, user_id, trophy_id, awarded_by, awarded_at)
4. guild_settings (guild_id, dedication_display, stack_roles, hide_unused_trophies, hide_quit_users, leaderboard_format)
5. role_rewards (id, guild_id, role_id, requirement, created_at)
6. leaderboard_panels (guild_id, channel_id, message_id, created_at)
7. bot_stats (key, value, updated_at)

**Key Constraints:**
- Trophies: UNIQUE(guild_id, name), value BETWEEN -999999 AND 999999
- Role rewards: UNIQUE(guild_id, role_id), requirement >= 1
- User trophies: Many-to-many with trophies, ON DELETE CASCADE

**See migrations/001_initial_schema.sql for complete schema definition.

**Performance Optimizations:**
- Index on (guild_id, user_id) for leaderboard queries
- Index on (guild_id, name) for trophy lookup
- Index on (guild_id, awarded_at DESC) for award history
- Index on (guild_id, requirement) for role rewards

**Migration Benefits:**
- **No more 150-trophy limit** - database can handle millions of trophies
- **Efficient queries** - Only fetch needed data, not entire JSON blobs
- **Concurrent operations** - Multiple servers can operate simultaneously
- **Proper relationships** - Foreign key constraints ensure data integrity

**Performance Benefits:**
- **Indexed lookups** - O(log n) instead of O(n) JSON parsing
- **Memory efficiency** - Load only required data into memory  
- **Cache-friendly** - Individual queries can be cached effectively
- **Batch operations** - Award multiple trophies in single transaction

## Data Migration Strategy

**Current:** quick.db stores ALL data as JSON in 2 SQLite tables (`bot`, `guilds` - single row each)
**Target:** Normalized PostgreSQL with proper tables and relationships

**Production Data Volume (critical for migration planning):**
- 2,493 guilds
- 10,571 trophies total
- 120,411 trophy awards
- 104,913 commands executed

**Migration Challenges:**
1. Trophy ID remapping (string IDs → auto-increment integers)
2. User trophy arrays → normalized user_trophies table
3. Legacy fields present in 95.7% of guilds (.restapi) but unused
4. No "awarded_by" tracking in current system

**Key Steps:**
1. Extract JSON from legacy SQLite
2. Parse into Rust structs
3. Migrate to normalized PostgreSQL in single transaction
4. Validate counts and scores match exactly
5. Test with production data copy before cutover

**See MIGRATION_GUIDE.md for detailed implementation code and scripts (when needed for actual migration).

## Product Functionality & User Experience

### Core User Journeys

**New Member Experience:**
1. **Passive Entry**: Joins server, no setup required
2. **First Recognition**: Receives trophy notification from admin/moderator
3. **Discovery**: Uses `/trophies user` to see their collection
4. **Competition**: Checks `/leaderboard` to see their ranking
5. **Motivation**: Earns role rewards as score increases
    emoji: String,
    value: i32,
    image: Option<String>,
    dedication: Option<LegacyDedication>,
    details: String,
    signed: bool,
}

#[derive(Deserialize)]
struct LegacyUser {
    trophies: Vec<i32>,        // Array of trophy IDs
    trophy_value: i64,         // Total score
}
```

### Migration Implementation

**Step 1: Extract and Validate Legacy Data**
```rust
async fn extract_legacy_data(legacy_db_path: &str) -> Result<(LegacyBotData, LegacyGuildData)> {
    // Connect to legacy SQLite database
    let pool = SqlitePool::connect(legacy_db_path).await?;
    
    // Extract bot data JSON
    let bot_json: String = sqlx::query_scalar!(
        "SELECT json FROM bot WHERE id = 'data'"
    ).fetch_one(&pool).await?;
    
    // Extract guilds data JSON  
    let guilds_json: String = sqlx::query_scalar!(
        "SELECT json FROM guilds WHERE id = 'data'" 
    ).fetch_one(&pool).await?;
    
    // Parse JSON into structs
    let bot_data: LegacyBotData = serde_json::from_str(&bot_json)?;
    let guild_data: LegacyGuildData = serde_json::from_str(&guilds_json)?;
    
    // Validate data integrity
    validate_legacy_data(&bot_data, &guild_data).await?;
    
    Ok((bot_data, guild_data))
}
```

**Step 2: Create Migration Transaction**
```rust
async fn migrate_to_normalized_db(
    legacy_data: (LegacyBotData, LegacyGuildData),
    target_pool: &PgPool
) -> Result<MigrationReport> {
    let mut tx = target_pool.begin().await?;
    let mut report = MigrationReport::new();
    
    // Step 2a: Migrate global bot stats
    migrate_bot_stats(&legacy_data.0, &mut tx, &mut report).await?;
    
    // Step 2b: Migrate guilds and their data
    for (guild_id_str, guild_data) in legacy_data.1.guilds {
        let guild_id: i64 = guild_id_str.parse()?;
        
        // Insert guild
        migrate_guild(guild_id, &guild_data, &mut tx, &mut report).await?;
        
        // Migrate guild settings
        migrate_guild_settings(guild_id, &guild_data, &mut tx, &mut report).await?;
        
        // Migrate trophies
        let trophy_mapping = migrate_trophies(guild_id, &guild_data.trophies, &mut tx, &mut report).await?;
        
        // Migrate user trophy awards
        migrate_user_trophies(guild_id, &guild_data.users, &trophy_mapping, &mut tx, &mut report).await?;
        
        // Migrate role rewards
        migrate_role_rewards(guild_id, &guild_data, &mut tx, &mut report).await?;
        
        // Migrate leaderboard panels
        if let Some(panel) = &guild_data.panel {
            migrate_panel(guild_id, panel, &mut tx, &mut report).await?;
        }
    }
    
    // Commit everything or rollback on error
    tx.commit().await?;
    Ok(report)
}
```

**Step 3: Handle Trophy ID Remapping**
```rust
async fn migrate_trophies(
    guild_id: i64,
    legacy_trophies: &HashMap<String, LegacyTrophy>,
    tx: &mut Transaction<'_, Postgres>,
    report: &mut MigrationReport
) -> Result<HashMap<i32, i32>> { // old_id -> new_id mapping
    
    let mut id_mapping = HashMap::new();
    
    for (old_id_str, trophy) in legacy_trophies {
        // Skip the "current" counter key
        if old_id_str == "current" { continue; }
        
        let old_id: i32 = old_id_str.parse()?;
        
        // Insert trophy and get new auto-generated ID
        let new_id = sqlx::query_scalar!(
            r#"
            INSERT INTO trophies (
                guild_id, creator_user_id, name, description, emoji, 
                value, image_filename, dedication_user_id, dedication_text,
                details, signed, created_at
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
            RETURNING id
            "#,
            guild_id,
            trophy.creator.parse::<i64>().unwrap_or(0),
            trophy.name,
            trophy.description,
            trophy.emoji,
            trophy.value,
            trophy.image,
            trophy.dedication.as_ref().and_then(|d| d.user.as_ref()?.parse().ok()),
            trophy.dedication.as_ref().map(|d| d.name.clone()),
            trophy.details,
            trophy.signed,
            // Convert Unix timestamp to DateTime
            DateTime::from_timestamp(trophy.created / 1000, 0).unwrap_or_else(Utc::now)
        ).fetch_one(&mut **tx).await?;
        
        id_mapping.insert(old_id, new_id);
        report.trophies_migrated += 1;
    }
    
    Ok(id_mapping)
}
```

**Step 4: Migrate User Trophy Awards**
```rust
async fn migrate_user_trophies(
    guild_id: i64,
    legacy_users: &HashMap<String, LegacyUser>,
    trophy_mapping: &HashMap<i32, i32>,
    tx: &mut Transaction<'_, Postgres>,
    report: &mut MigrationReport
) -> Result<()> {
    
    for (user_id_str, user_data) in legacy_users {
        let user_id: i64 = user_id_str.parse()?;
        
        for &legacy_trophy_id in &user_data.trophies {
            if let Some(&new_trophy_id) = trophy_mapping.get(&legacy_trophy_id) {
                // Insert user trophy award
                sqlx::query!(
                    r#"
                    INSERT INTO user_trophies (guild_id, user_id, trophy_id, awarded_by, awarded_at)
                    VALUES ($1, $2, $3, $4, NOW())
                    "#,
                    guild_id,
                    user_id,
                    new_trophy_id,
                    0i64  // Unknown who awarded (legacy data doesn't track this)
                ).execute(&mut **tx).await?;
                
                report.user_awards_migrated += 1;
            } else {
                report.orphaned_awards += 1;
                eprintln!("Warning: User {} has trophy {} that doesn't exist", user_id, legacy_trophy_id);
            }
        }
    }
    
    Ok(())
}
```

### Migration Validation

**Data Integrity Checks:**
```rust
#[derive(Debug)]
struct MigrationReport {
    guilds_migrated: i32,
    trophies_migrated: i32,
    user_awards_migrated: i32,
    role_rewards_migrated: i32,
    orphaned_awards: i32,
    errors: Vec<String>,
}

async fn validate_migration(
    legacy_data: &(LegacyBotData, LegacyGuildData),
    new_pool: &PgPool
) -> Result<ValidationReport> {
    
    // Verify trophy counts match
    let legacy_trophy_count: i32 = legacy_data.1.guilds.values()
        .map(|g| g.trophies.len() - 1) // -1 for "current" key
        .sum::<usize>() as i32;
        
    let new_trophy_count: i32 = sqlx::query_scalar!(
        "SELECT COUNT(*) FROM trophies"
    ).fetch_one(new_pool).await?;
    
    assert_eq!(legacy_trophy_count, new_trophy_count, "Trophy count mismatch");
    
    // Verify user score calculations
    for (guild_id_str, guild_data) in &legacy_data.1.guilds {
        let guild_id: i64 = guild_id_str.parse()?;
        
        for (user_id_str, user_data) in &guild_data.users {
            let user_id: i64 = user_id_str.parse()?;
            
            // Calculate expected score from new DB
            let calculated_score: Option<i64> = sqlx::query_scalar!(
                r#"
                SELECT COALESCE(SUM(t.value), 0)
                FROM user_trophies ut
                JOIN trophies t ON ut.trophy_id = t.id  
                WHERE ut.guild_id = $1 AND ut.user_id = $2
                "#,
                guild_id,
                user_id
            ).fetch_one(new_pool).await?;
            
            assert_eq!(
                user_data.trophy_value,
                calculated_score.unwrap_or(0),
                "Score mismatch for user {} in guild {}", user_id, guild_id
            );
        }
    }
    
    Ok(ValidationReport::success())
}
```

### Migration Safety & Rollback

**Backup Strategy:**
```bash
# 1. Backup original JSON SQLite
cp json.sqlite json.sqlite.backup.$(date +%Y%m%d_%H%M%S)

# 2. Export readable JSON for manual verification  
sqlite3 json.sqlite "SELECT json FROM guilds WHERE id='data'" > guilds_backup.json
sqlite3 json.sqlite "SELECT json FROM bot WHERE id='data'" > bot_backup.json

# 3. Create PostgreSQL dump after migration
pg_dump trophy_bot > migration_result.$(date +%Y%m%d_%H%M%S).sql
```

**Rollback Plan:**
```rust
async fn rollback_migration(backup_path: &str, target_pool: &PgPool) -> Result<()> {
    // Drop all tables and recreate from backup
    sqlx::query!("DROP SCHEMA public CASCADE").execute(target_pool).await?;
    sqlx::query!("CREATE SCHEMA public").execute(target_pool).await?;
    
    // Restore from SQL backup
    let backup_sql = std::fs::read_to_string(backup_path)?;
    sqlx::query(&backup_sql).execute(target_pool).await?;
    
    Ok(())
}
```

### Image File Migration

**Trophy Images:**
```rust
async fn migrate_trophy_images(guild_id: i64, report: &mut MigrationReport) -> Result<()> {
    let legacy_image_dir = "./images";
    let new_image_dir = "./assets/trophy_images";
    
    // Ensure new directory exists
    tokio::fs::create_dir_all(new_image_dir).await?;
    
    // Find all images for this guild  
    let pattern = format!("{}_{}_*", guild_id, "*");
    
    for entry in glob(&format!("{}/{}", legacy_image_dir, pattern))? {
        let old_path = entry?;
        let filename = old_path.file_name().unwrap();
        let new_path = Path::new(new_image_dir).join(filename);
        
        // Copy file to new location
        tokio::fs::copy(&old_path, &new_path).await?;
        report.images_migrated += 1;
    }
    
    Ok(())
}
```

### Migration Execution Plan

**Realistic Production Migration Strategy:**

**Phase 1: Testing & Validation**
1. **Create production data copy**: `cp json.sqlite test_migration.sqlite`
2. **Run migration on test copy**: Full migration to test PostgreSQL instance
3. **Deploy test bot instance**: New Rust bot with migrated test data
4. **Thorough testing**: Verify all commands work with migrated data
5. **Performance validation**: Confirm improved response times and scalability

**Phase 2: Production Migration**
1. **Scheduled maintenance window**: Announce bot downtime (30-60 minutes)
2. **Final backup**: Create timestamped backup of production data
3. **Execute migration**: Run tested migration scripts on production data
4. **Deploy new bot**: Replace Node.js bot with validated Rust version
5. **Immediate validation**: Test core functions (create trophy, award, leaderboard)
6. **Monitor**: Watch for errors and performance in first hours

**Phase 3: Post-Migration**
1. **User communication**: Announce successful migration and new capabilities
2. **Monitor performance**: Track improved response times and reduced errors
3. **Archive legacy system**: Keep backups but remove active Node.js deployment
4. **Remove artificial limits**: Increase trophy limits, enable new features

**Rollback Plan (if needed):**
- Keep Node.js version ready for quick deployment
- Restore from SQLite backup if critical issues found
- Rollback window: First 24 hours post-migration

**Key Advantages of This Approach:**
- ✅ **Single cutover**: No complex dual-system management
- ✅ **Thorough testing**: Real production data tested extensively  
- ✅ **Fast rollback**: Simple revert to working Node.js version
- ✅ **Minimal complexity**: Straightforward migration path
- ✅ **Clear success criteria**: Easy to validate migration worked

**Expected Downtime:** 30-60 minutes (vs. days/weeks of dual-system complexity)

## Product Functionality & User Experience

### Core User Journeys

**New Member Experience:**
1. **Passive Entry**: Joins server, no setup required
2. **First Recognition**: Receives trophy notification from admin/moderator
3. **Discovery**: Uses `/trophies user` to see their collection
4. **Competition**: Checks `/leaderboard` to see their ranking
5. **Motivation**: Earns role rewards as score increases
6. **Continued Engagement**: Participates more to improve ranking

**Server Administrator Experience:**
1. **Bot Setup**: Invite bot, run `/imsafe` to enable management
2. **Trophy Design**: Create custom trophies with `/create` (images, values, descriptions)
3. **Permission Config**: Set Discord slash command permissions for managers
4. **Role Rewards**: Configure automatic role progression with `/rewards`
5. **Daily Management**: Award trophies with `/award`, monitor leaderboards
6. **Community Tuning**: Adjust settings, edit trophies, manage panel displays

### Key Product Features

**Recognition System:**
- **Custom Trophies**: Unique server-specific achievements with images and lore
- **Flexible Values**: Positive, negative, or zero-point trophies for different recognition types
- **Bulk Awards**: Award up to 50 trophies at once for events or achievements
- **Dedication Messages**: Personalize trophy awards with custom messages

**Gamification Elements:**
- **Server Leaderboards**: Public rankings with medals for top 3 positions
- **Score Progression**: Cumulative points from all trophies earned
- **Role Rewards**: Automatic Discord role assignment based on score thresholds (up to 20 tiers)
- **Trophy Collections**: Personal achievement galleries viewable by all members

**Administrative Controls:**
- **Permission Layers**: Separate manage-trophies vs award-trophies permissions
- **Display Settings**: Hide unused trophies, quit users, customize leaderboard format
- **Safety Systems**: `/imsafe` flag prevents accidental permission escalation
- **Data Management**: Complete server data export/deletion capabilities

**Social Features:**
- **Public Recognition**: Trophy awards create community moments
- **Persistent Panels**: Auto-updating leaderboard displays in channels
- **Trophy Browsing**: View server trophy catalog with `/trophies guild`
- **Achievement Sharing**: Trophy details visible to all with descriptions and images

### Error Handling & Edge Cases

**User-Facing Errors:**
- Friendly error messages with support server links
- Graceful handling of missing permissions or invalid inputs
- Clear validation feedback for text lengths and file sizes

**Administrative Safeguards:**
- Owner-only actions for sensitive operations like `/forgetme`
- Resource limits (150 trophies per server) prevent database bloat
- Input validation prevents malformed data entry
- Permission checks respect Discord role hierarchy

**Technical Resilience:**
- Automatic error logging to developer channels
- Database corruption prevention with input sanitization
- Image storage failure handling with graceful fallbacks
- Service recovery with background task monitoring

### Privacy & Data Management

**Data Collection:**
- Server IDs, user IDs (no personal information)
- Trophy definitions, awards, and scores
- Custom trophy images stored locally
- Aggregate usage statistics only

**User Rights:**
- View personal trophy data anytime
- Server admins control all server data
- Complete data deletion with `/forgetme`
- No cross-server data sharing

**Compliance Features:**
- Minimal data collection (only functional necessities)
- Clear data deletion mechanisms
- Error logs auto-deleted after resolution
- Support server for privacy requests

## Testing

No formal test suite is configured. Manual testing involves:
1. Setting up environment with `.env` file
2. Running bot in a test Discord server
3. Testing slash commands functionality

## Node.js Version

Requires Node.js >=18 <19 (specified in package.json engines field).

## Additional Documentation Files

Read @DISCORD_COMMANDS_DOCUMENTATION.md comprehensive technical specifications for all 26 Discord slash commands
  including exact parameters, validation rules, SlashCommandBuilder structures, business logic, database operations,
  and implementation details for the Rust rewrite.

En los archivos @bot_db.json y guilds_db.json tienes los 2 JSONs que guarda en la DB SQLite para que puedas buscar
  y ver como funciona la estructura de esos JSONs.

Registered guild command: about
Registered guild command: forgetme
Registered guild command: help
Registered guild command: imsafe
Registered guild command: invite
Registered guild command: ping
Registered guild command: stats
Registered guild command: suggest
Registered guild command: support
Registered guild command: award
Registered guild command: clear
Registered guild command: create
Registered guild command: delete
Registered guild command: details
Registered guild command: edit
Registered guild command: panel
Registered guild command: permissions
Registered guild command: revoke
Registered guild command: rewards
Registered guild command: settings
Registered guild command: leaderboard
Registered guild command: show
Registered guild command: trophies
Registered guild command: export

## Comandos del Bot de Discord
- `/about` - Who am I? Who are you? Questions never asked.
- `/help` - Stop it! Get some help!
- `/ping` - Current bot ping! If the bot doesn't answer then ping is probably over 5000ms and very likely down
- `/stats` - Look at the bot stats
- `/support` - You need extra help? Join our support server.
- `/suggest` - Suggest a feature or change for the bot. (Now just an advice to join the support server to suggest)
- `/award` - Award a trophy for an user.
- `/clear` - Clear all trophies and resets the score of an user to 0.
- `/create` - Create a new trophy for your server.
- `/delete` - Delete a trophy from your server.
- `/details` - Shows the details of a trophy
- `/edit` - Edit an existing trophy for your server.
- `/revoke` - Revoke a trophy from an user.
- `/show` - Show a trophy.
- `/trophies guild` - Show the trophies any guild has.
- `/trophies user` - Show the trophies any user has.
- `/leaderboard` - Shows the server's leaderboard.
- `/panel create` - Create the panel for the leaderboard.
- `/panel delete` - Delete the panel for the leaderboard.
- `/rewards add` - Add permissions to a role.
- `/rewards clear` - Clears all rewards in this server.
- `/rewards list` - List of reward roles.
- `/rewards remove` - Remove a role reward from your server.
- `/permissions add` - Add permissions to a role.
- `/permissions list` - List all permissions.
- `/permissions remove` - Remove permissions from a role.
- `/imsafe` - Confirms you're using discord permissions instead of the deprecated custom permissions
- `/settings list` - List all settings of the server
- `/settings set` - Change a setting for the server.
- `/invite` - Invite the bot to your server!
- `/export` - Export the bot's data
- `/forgetme` - Remove all images and data about your server from the bot and kick it.
