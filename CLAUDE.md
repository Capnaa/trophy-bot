# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## 📚 Documentation Structure

This repository contains multiple documentation files for different purposes:

- **CLAUDE.md** (this file) - High-level project overview, architecture, and development guidelines
- **MIGRATION.md** - Detailed data migration strategy from Node.js/quick.db to Rust/SeaORM (production statistics, algorithms, validation)
- **COMMAND_IMPLEMENTATIONS.md** - Quick reference mapping each slash command to its Node.js implementation file
- **DISCORD_COMMANDS_DOCUMENTATION.md** - Comprehensive command specifications with parameters, validation rules, and business logic
- **COMMANDS_AND_FUNCTIONALITY.md** - Summary of bot commands, events, and data structures

**When implementing Rust commands:** Refer to DISCORD_COMMANDS_DOCUMENTATION.md for full specifications, then check COMMAND_IMPLEMENTATIONS.md to find the corresponding Node.js file for implementation details.

## Project Overview

Trophy Bot is a **gamification and community recognition system** for Discord servers. It transforms subjective appreciation into a structured achievement system where:

- **Server admins** create custom trophies with names, descriptions, images, and point values
- **Members** receive trophies for contributions and see their progress on leaderboards  
- **Communities** benefit from increased engagement through competition and role progression
- **Recognition** becomes visible, measurable, and rewarding for all participants

The bot solves the problem of informal appreciation by creating tangible rewards, social status, and competitive motivation within Discord communities.

Always after change code, run cargo test to ensure no errors.

For Rust code:
- Does not use println or eprintln instead use log crate
- Try to avoid to put code in main function, it must be modular using modules and functions

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

### IMPORTANT: Non-functional Code

**Files that DON'T work/aren't used:**
- `/commons/database.js` - Defines mongoose functions but mongoose isn't used
- `/commons/models.js` - Mongoose models that aren't used
- `/commons/schemas.js` - Mongoose schemas that aren't used  
- `/commons/configs.js` - Stub file, real config is in globals.js
- `/commons/locale.js` - Empty stub, locale loading happens in globals.js
- `/commons/utils.js` - Empty file
- `/TrophyBot-Copy/` - Backup directory, ignore completely
- `/windows/` - UI window classes, mostly empty or deprecated
- **Language system**: Partially implemented but disabled (commented out in commands)

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

### Essential Functions (from globals.js)

**Core Utilities:**
- `getPage(array, perPage, currentPage)` - Pagination logic, returns {list, page, last}
- `parseUser(client, input, fallback, guild, fetchFromDiscord)` - Parse user mentions/IDs/names
- `downloadImage(url, path)` - Save trophy images using node-fetch
- `getSetting(client, guild, setting)` - Get guild configuration with defaults
- `getTrophyCount(client, guild)` - Count guild's trophies (excluding 'current' key)
- `showError(message)` - Format error messages with emoji
- `imsafeWarning(interaction)` - Safety mode migration warning
- `getServer(client, guildId, guild)` - Initialize/get guild data structure
- `getDedication(guild, dedication, config)` - Format trophy dedication text
- `isInServer(guild, user)` - Check if user is still in server
- `anyIn(array1, array2)` - Check if any element from array1 exists in array2
- `isDev(userId)` - Check if user is bot developer (hardcoded IDs)

**Background Tasks:**
- `fetchModules(directory, extension, recursive)` - Load command/language files
- `fixShit(client)` - Database initialization and migration
- `changeActivity(client)` - Periodic status updates
- `updatePanels(client)` - Update leaderboard panels
- `AttemptToFetchUsers(client, force)` - Update user count statistics

### Complete Command Reference

**📋 Detailed Implementation Guide:** See @DISCORD_COMMANDS_DOCUMENTATION.md for comprehensive technical specifications of all commands including exact parameters, validation rules, and implementation details.

**All Commands (26 total) with Exact Descriptions:**

**Bot Utility Commands:**
- `/about` - Who am I? Who are you? Questions never asked.
- `/forgetme` - Remove all images and data about your server from the bot and kick it.
- `/help` - Stop it! Get some help!
- `/imsafe` - Confirms you're using Discord permissions instead of the deprecated custom permissions.
- `/invite` - Invite the bot to your server!
- `/ping` - Current bot ping! If the bot doesn't answer then ping is probably over 5000ms and very likely down.
- `/stats` - Look at the bot stats
- `/suggest` - Suggest a feature or change for the bot
- `/support` - You need extra help? Join our support server

**Trophy Management Commands:**
- `/award` - Award a trophy for an user.
- `/clear` - Clear all trophies and resets the score of an user to 0.
- `/create` - Create a new trophy for your server.
- `/delete` - Delete a trophy from your server.
- `/details` - Shows the details of a trophy.
- `/edit` - Edit an existing trophy for your server.
- `/revoke` - Revoke a trophy from an user.

**Server Administration Commands:**
- `/export` - Export the bot's data.
- `/panel create` - Create the panel for the leaderboard.
- `/panel delete` - Delete the panel for the leaderboard.
- `/permissions add` - Add permissions to a role. (DEPRECATED)
- `/permissions list` - List all permissions. (DEPRECATED) 
- `/permissions remove` - Remove permissions from a role. (DEPRECATED)
- `/rewards add` - Add permissions to a role.
- `/rewards clear` - Clears all rewards in this server
- `/rewards list` - List of reward roles
- `/rewards remove` - Remove a role reward from your server
- `/settings list` - List all settings of the server
- `/settings set` - Change a setting for the server

**User-Facing Commands:**
- `/leaderboard` - Shows the server's leaderboard.
- `/show` - Show a trophy
- `/trophies guild` - Show the trophies any guild has
- `/trophies user` - Show the trophies any user has

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

### Rust Architecture (ACTUAL IMPLEMENTATION)

**Technology Stack (Current):**
- **Discord API**: [Serenity](https://github.com/serenity-rs/serenity) v0.12 + [Poise](https://github.com/serenity-rs/poise) v0.6
- **Database ORM**: [SeaORM](https://www.sea-ql.org/SeaORM/) v2.0-rc with migrations
- **Database Support**: SQLite (development) + PostgreSQL (production)
- **CLI**: clap v4 with derive features for subcommands
- **Async Runtime**: Tokio with multi-threaded flavor

**Key Advantages Over Node.js Version:**
- **Type-safe ORM** - SeaORM provides compile-time checked queries with ActiveModel pattern
- **Migration System** - Built-in migration management via `cargo run -- migrate up/down/status`
- **True async/await** with Rust's superior async runtime (Tokio)
- **Memory safety** - No garbage collection, predictable resource usage
- **Performance** - 10-100x faster than Node.js for database operations
- **Scalability** - Handle thousands of concurrent Discord interactions

**Current Implementation Status:**
- ✅ CLI structure with migration subcommands (`src/cli.rs`, `src/migrations/mod.rs`)
- ✅ Legacy data loader from quick.db (`src/legacy/mod.rs`)
- ✅ SeaORM migration infrastructure (`src/migrations/`)
- ✅ Bot framework with Serenity + Poise (`src/bot/mod.rs`)
- ⚠️ Basic migration schema exists but needs completion
- ❌ No production commands implemented yet (only demo `bench` command)
- ❌ Data migration script not yet implemented

**Project Structure:**
```
src/
├── main.rs              # Entry point, routes to bot or migration
├── cli.rs               # CLI argument parsing with migrate subcommand
├── bot/
│   ├── mod.rs           # Bot initialization and framework setup
│   └── commands.rs      # Poise command implementations
├── legacy/
│   └── mod.rs           # Loads JSON from quick.db SQLite (production data)
└── migrations/
    ├── mod.rs           # SeaORM Migrator trait implementation
    └── m20251115_000001_create_basic_tables.rs  # Example migration
```

## Normalized Database Schema (SeaORM)

**⚠️ IMPORTANT:** The actual schema is defined in `migrations/001_initial_schema.sql` and managed via SeaORM migrations.
**See `MIGRATION.md`** for the complete migration strategy including legacy_id mapping and data import algorithm.

### Database Schema Overview

**Schema Management:**
- Migrations live in `src/migrations/` as Rust files using SeaORM's migration API
- Run migrations: `cargo run -- migrate up`
- Check status: `cargo run -- migrate status`
- Rollback: `cargo run -- migrate down`

**Core Tables (7 total):**

1. **guilds** - Discord server configurations
2. **trophies** - Trophy definitions with legacy_id mapping
3. **user_trophies** - Award records (allows duplicates for multiple awards)
4. **guild_settings** - Per-guild configuration (5 settings)
5. **role_rewards** - Automatic role assignment rules
6. **leaderboard_panels** - Persistent leaderboard messages
7. **bot_stats** - Global statistics and command counters

**Key Design Decisions:**
- `trophies.legacy_id` - Maps old string IDs ("1", "2") to new SERIAL integers
- NO UNIQUE constraint on `(user_id, trophy_id)` - duplicates are required functionality
- `user_trophies.awarded_at` - Synthetic timestamps for migrated data (legacy system didn't track)
- Soft deletes with `deleted_at` columns (Laravel-style)
- Foreign keys with CASCADE for automatic cleanup

**Schema Definition (Example from migrations/001_initial_schema.sql):**
```sql
-- Guilds table
CREATE TABLE guilds (
    id BIGINT PRIMARY KEY,
    name VARCHAR(100),
    is_safe BOOLEAN DEFAULT FALSE,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    deleted_at TIMESTAMP NULL
);

-- Trophies with legacy ID mapping
CREATE TABLE trophies (
    id SERIAL PRIMARY KEY,
    guild_id BIGINT NOT NULL,
    creator_user_id BIGINT NOT NULL,
    name VARCHAR(32) NOT NULL,
    description VARCHAR(128) DEFAULT 'No description provided',
    emoji VARCHAR(64) DEFAULT '🏆',
    value INTEGER CHECK (value BETWEEN -999999 AND 999999) DEFAULT 10,
    image_filename VARCHAR(255) NULL,
    dedication_user_id BIGINT NULL,
    dedication_text VARCHAR(32) NULL,
    details VARCHAR(300) DEFAULT 'No details provided.',
    signed BOOLEAN DEFAULT FALSE,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    deleted_at TIMESTAMP NULL,

    CONSTRAINT fk_trophies_guild FOREIGN KEY (guild_id) REFERENCES guilds(id) ON DELETE CASCADE,
    CONSTRAINT unique_guild_trophy_name UNIQUE(guild_id, name)
);

-- User trophy awards (DUPLICATES ALLOWED)
CREATE TABLE user_trophies (
    id SERIAL PRIMARY KEY,
    guild_id BIGINT NOT NULL,
    user_id BIGINT NOT NULL,
    trophy_id INTEGER NOT NULL,
    awarded_by BIGINT NOT NULL,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    deleted_at TIMESTAMP NULL,

    CONSTRAINT fk_user_trophies_guild FOREIGN KEY (guild_id) REFERENCES guilds(id) ON DELETE CASCADE,
    CONSTRAINT fk_user_trophies_trophy FOREIGN KEY (trophy_id) REFERENCES trophies(id) ON DELETE CASCADE
    -- NO UNIQUE CONSTRAINT - duplicates required for multiple awards
);
```

**See `migrations/001_initial_schema.sql` for the complete schema including:**
- guild_settings table (5 configuration options)
- role_rewards table (automatic role assignment)
- leaderboard_panels table (persistent leaderboard messages)
- bot_stats table (command counters and statistics)
- command_logs table (analytics and debugging)
- Indexes for performance optimization
- Triggers for updated_at maintenance

### SeaORM Implementation Patterns

**Active Model Pattern (Create/Update):**
```rust
use sea_orm::{ActiveModelTrait, Set};
use entities::{guilds, trophies};

// Insert a new trophy
let new_trophy = trophies::ActiveModel {
    guild_id: Set(guild_id),
    name: Set("Golden Medal".to_string()),
    description: Set("Awarded for excellence".to_string()),
    value: Set(100),
    ..Default::default()
};

let inserted = new_trophy.insert(&db).await?;
let trophy_id = inserted.id;
```

**Entity Queries (Read):**
```rust
use sea_orm::{EntityTrait, QueryFilter, QuerySelect, ColumnTrait};
use entities::{user_trophies, trophies};

// Find all user trophies with trophy details
let user_awards = user_trophies::Entity::find()
    .filter(user_trophies::Column::UserId.eq(user_id))
    .filter(user_trophies::Column::GuildId.eq(guild_id))
    .find_also_related(trophies::Entity)
    .all(&db)
    .await?;

// Calculate user score
use sea_orm::sea_query::{Expr, Func};

let score: Option<i64> = user_trophies::Entity::find()
    .select_only()
    .column_as(Expr::col(trophies::Column::Value).sum(), "total")
    .filter(user_trophies::Column::UserId.eq(user_id))
    .filter(user_trophies::Column::GuildId.eq(guild_id))
    .join(JoinType::InnerJoin, user_trophies::Relation::Trophies)
    .into_tuple::<i64>()
    .one(&db)
    .await?;
```

**Transaction Example:**
```rust
use sea_orm::TransactionTrait;

let txn = db.begin().await?;

// Award trophy
let award = user_trophies::ActiveModel {
    guild_id: Set(guild_id),
    user_id: Set(user_id),
    trophy_id: Set(trophy_id),
    ..Default::default()
};
award.insert(&txn).await?;

// Update role rewards based on new score
update_role_rewards(&txn, guild_id, user_id).await?;

txn.commit().await?;
```

### Migration Benefits

**Scalability Improvements:**
- **No more 150-trophy limit** - database can handle millions of trophies
- **Efficient queries** - Only fetch needed data, not entire JSON blobs
- **Concurrent operations** - Multiple servers can operate simultaneously
- **Proper relationships** - Foreign key constraints ensure data integrity

**Performance Benefits:**
- **Indexed lookups** - O(log n) instead of O(n) JSON parsing
- **Memory efficiency** - Load only required data into memory  
- **Cache-friendly** - Individual queries can be cached effectively
- **Batch operations** - Award multiple trophies in single transaction

## Data Migration Strategy (JSON SQLite → Normalized DB)

**⚠️ IMPORTANT:** This section provides a brief overview. For the complete migration strategy, see `MIGRATION.md` which contains:
- Verified production statistics (2,493 guilds, 10,853 trophies, 60,554 awards)
- 5 critical issues and their solutions
- Complete PostgreSQL schema with SeaORM
- 8-phase migration algorithm with SeaORM Active Models
- Validation strategy and rollback plan

### Migration Overview

**Current Implementation:**
- Legacy data loader: `src/legacy/mod.rs` reads from `sqlite://json.sqlite`
- Exposes `LegacyData` struct with `.bot()` and `.guilds()` methods
- Serde JSON deserialization for type-safe access

**Critical Challenge**: The current system stores ALL data as JSON in 2 SQLite tables:
```sql
-- Current structure (quick.db)
bot: {"data": {"version": 0, "commands": {...}, "trophies": 123, ...}}
guilds: {"data": {"guild1": {"trophies": {...}, "users": {...}}, "guild2": {...}}}
```

**Target**: Normalize into 7 tables with SeaORM entities and proper relationships.

### Pre-Migration Analysis

**Read Current Data Structure:**
```rust
// Parse the existing SQLite JSON blobs
#[derive(Deserialize)]
struct LegacyBotData {
    version: i32,
    commands: HashMap<String, i64>,
    trophies: i64,
    trophies_awarded: i64,
    banned_users: Vec<i64>,
}

#[derive(Deserialize)]
struct LegacyGuildData {
    #[serde(flatten)]
    guilds: HashMap<String, LegacyGuild>,
}

#[derive(Deserialize)]
struct LegacyGuild {
    imsafe: Option<bool>,
    language: Option<String>,
    settings: Option<HashMap<String, i32>>,
    trophies: HashMap<String, LegacyTrophy>,
    users: HashMap<String, LegacyUser>,
    rewards: Option<Vec<LegacyReward>>,
    permissions: Option<HashMap<String, Vec<String>>>,
    panel: Option<LegacyPanel>,
}

#[derive(Deserialize)]  
struct LegacyTrophy {
    creator: String,
    created: i64,
    name: String,
    description: String,
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
