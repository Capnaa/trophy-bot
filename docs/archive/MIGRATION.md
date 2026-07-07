# Migration Guide: Trophy Bot Node.js → Rust

**Document Version:** 1.0
**Date:** 2025-10-15
**Status:** Analysis Complete - Ready for Implementation

---

## Table of Contents

1. [Executive Summary](#executive-summary)
2. [Production Data Analysis](#production-data-analysis)
3. [Critical Issues Found](#critical-issues-found)
4. [Data Structure Documentation](#data-structure-documentation)
5. [PostgreSQL Schema Design](#postgresql-schema-design)
6. [Migration Algorithm](#migration-algorithm)
7. [Validation Strategy](#validation-strategy)
8. [Implementation Decisions](#implementation-decisions)

---

## Executive Summary

### Current System
- **Platform:** Node.js v18 with Discord.js v14
- **Database:** quick.db (SQLite with JSON blobs)
- **Structure:** 2 tables, each with single JSON row containing all data
- **Status:** Active production (100% traffic)

### Target System
- **Platform:** Rust with Serenity 0.12 + Poise 0.6
- **Database:** PostgreSQL with normalized schema
- **Status:** In development (0% traffic)

### Migration Approach
- **Type:** Single-cutover with full data migration
- **Downtime:** 30-60 minutes (estimated)
- **Rollback Plan:** Keep Node.js version ready for 24 hours

---

## Production Data Analysis

### Verified Statistics (from guilds_db.json)

| Metric | Value | Source |
|--------|-------|--------|
| **Active Guilds** | 2,493 | Actual count |
| **Total Trophies Created** | 10,853 | Actual count |
| **Current Trophy Awards** | 60,554 | Actual count |
| **Total Users with Trophies** | 8,299 | Actual count |
| **Users with Empty Arrays** | 1,284 | Actual count |
| **Commands Executed (Historic)** | 104,913 | bot_db.json |

### ⚠️ Data Integrity Issues Found

**Counter Desynchronization:**
- `bot_db.json` reports 10,571 trophies → **INCORRECT** (real: 10,853)
- `bot_db.json` reports 120,411 awards → **INCORRECT** (real: 60,554)
- **Cause:** Counters are cumulative (never decremented on revoke/clear/delete)
- **Impact:** Cannot use bot_db.json for validation

**Missing Fields:**
- 43 trophies (0.4%) missing `creator`, `created`, `signed`, `details`
- These are legacy trophies from older bot version
- Require default values during migration

**Data Distribution:**
- 393 guilds (15.8%) have zero trophies
- 1,284 users (13.4%) have empty trophy arrays
- Maximum trophies per user: 2,009 (legitimate, verified)
- Only 1 guild (0.04%) has a leaderboard panel configured

---

## Critical Issues Found

### 🚨 Issue #1: Bug in revoke.js (CRITICAL)

**Location:** `commands/manage/revoke.js:64`

**Code:**
```javascript
while (n > 0){
    trophies.pop(id);  // ← BUG: Array.pop() doesn't accept parameters
    n--;
}
```

**Problem:**
- `Array.pop()` in JavaScript ignores parameters
- Always removes the LAST element, regardless of trophy ID
- Example with array `["1", "5", "10", "1", "2"]`, revoking 2 of trophy "1":
  - **Current behavior:** Removes "1" and "2" (last 2 elements) ❌
  - **Expected behavior:** Removes both "1" occurrences ✅

**Impact:** Users may have wrong trophies removed

**Migration Decision:** **FIX the bug** - implement correct behavior in Rust
```rust
// Correct implementation: remove N occurrences from the end
for _ in 0..count {
    if let Some(pos) = trophies.iter().rposition(|&x| x == trophy_id) {
        trophies.remove(pos);
    }
}
```

---

### 🚨 Issue #2: Trophy IDs are Strings (CRITICAL)

**Evidence:**
- JSON structure: `"trophies": {"1": {...}, "2": {...}, "150": {...}}`
- `getTrophy()` function always returns string keys
- `award.js:63` pushes string ID to array
- Highest ID in production: "212"

**Impact on Migration:**
- PostgreSQL will use `SERIAL` (integer auto-increment)
- Need mapping table: `HashMap<String, i64>` during migration
- Example: `{"1" → 5841, "2" → 5842, "150" → 6003}`

**Migration Requirements:**
1. Insert trophy → get new integer ID
2. Store mapping: `legacy_id (string) → new_id (integer)`
3. Use mapping when migrating user trophy arrays
4. Keep `legacy_id` column in DB for reference

---

### 🚨 Issue #3: Duplicate Trophy IDs Allowed (CRITICAL)

**Evidence from award.js:57-65:**
```javascript
const prev = client.db.guilds.get(`data.${guild}.users.${user}.trophies`) ?? [];
const value = object.value * count;
let n = count;
while (n > 0){
    prev.push(id);  // ← Allows duplicates
    n--;
}
```

**Behavior:**
- User trophy arrays can have same ID multiple times
- Example: `["1", "2", "1", "5"]` = 4 awards (trophy "1" twice)
- Each element represents one individual award

**Production Evidence:**
- User with 2,009 trophies has only 3 unique IDs
- User with 1,110 trophies has only 1 unique ID (awarded 1,110 times)

**Schema Requirement:**
```sql
CREATE TABLE user_trophies (
    id SERIAL PRIMARY KEY,
    user_id BIGINT,
    trophy_id INTEGER,
    -- NO UNIQUE constraint on (user_id, trophy_id)
    -- Duplicates are REQUIRED functionality
);
```

**Migration Logic:**
```
JSON: user.trophies = ["1", "2", "1", "5"]
→ 4 INSERT statements:
  1. (user_id, trophy_id=mapping["1"])
  2. (user_id, trophy_id=mapping["2"])
  3. (user_id, trophy_id=mapping["1"])  // Duplicate OK
  4. (user_id, trophy_id=mapping["5"])
```

---

### 🚨 Issue #4: trophyValue Desynchronization (HIGH)

**Current Implementation:**
- `award.js:68`: `client.db.guilds.add('trophyValue', value)` - manual increment
- `revoke.js:69`: `client.db.guilds.subtract('trophyValue', value)` - manual decrement
- `leaderboard.js:28`: Reads `trophyValue` directly without recalculation
- `doRewardRoles:199`: Uses `trophyValue` for role assignment

**Problem:**
- Stored value can become incorrect if award/revoke fails
- No recalculation from actual trophies
- Can affect role rewards and leaderboard positions

**Solution for Rust:**
```sql
-- DO NOT create trophyValue column
-- Always calculate on-the-fly:
SELECT
    user_id,
    COALESCE(SUM(t.value), 0) as total_score
FROM user_trophies ut
JOIN trophies t ON ut.trophy_id = t.id
WHERE ut.guild_id = $1 AND ut.user_id = $2
GROUP BY user_id;
```

**Benefits:**
- Always accurate
- No desynchronization possible
- Single source of truth

---

### 🚨 Issue #5: Bot Counters are Cumulative Only (MEDIUM)

**Analysis of counter updates:**
- `create.js:201`: Increments `bot.trophies` ✅
- `delete.js:42`: Decrements `bot.trophies` ✅
- `award.js:69`: Increments `bot.trophiesAwarded` ✅
- `revoke.js`: **DOES NOT** decrement `trophiesAwarded` ❌
- `clear.js`: **DOES NOT** decrement `trophiesAwarded` ❌

**Result:**
- `bot.trophies`: Mostly accurate (10,571 vs real 10,853 = -282 error)
- `bot.trophiesAwarded`: Severely inflated (120,411 vs real 60,554 = +59,857 error)

**Cause of Trophy Count Error:**
- Likely from failed creates that still incremented counter
- Or deletes that failed but still decremented

**Migration Decision:**
- **IGNORE** bot_db.json counters
- Recalculate all statistics from actual data post-migration

---

## Data Structure Documentation

### Trophy Object (JSON)

**Complete Structure:**
```json
{
  "creator": "353998390734094346",
  "created": 1655202620089,
  "name": "Trophy Name",
  "description": "Trophy description text",
  "emoji": "🏆",
  "value": 10,
  "image": "714504927329910847_16.png",
  "dedication": {
    "user": "780568061803102228",
    "name": "Leanled"
  },
  "details": "Private admin notes",
  "signed": true
}
```

**Field Presence Statistics:**
- `creator`: 99.6% (43 missing)
- `created`: 99.6% (43 missing)
- `signed`: 99.6% (43 missing)
- `details`: 96.7% (360 missing, 9,012 are default text)
- `image`: 26.6% (7,965 are null)
- `dedication`: 6.8% have non-empty (10,113 are empty `{}`)

**Dedication Variations:**
1. `{}` - No dedication
2. `{"user": null, "name": null}` - Equivalent to empty
3. `{"user": null, "name": "Free text"}` - Dedication to text
4. `{"user": "id", "name": "username"}` - Dedication to user

**Image Variations:**
1. `null` - No image
2. `"714504927329910847_16.png"` - Local file in `./images/`
3. `"https://cdn.discordapp.com/..."` - External URL

**Value Range:**
- Minimum: -999999 (found in production)
- Maximum: 999999 (found in production)
- Common negatives: -1, -10, -50, -100, -1000, -5000
- Validation: `value BETWEEN -999999 AND 999999`

---

### User Object (JSON)

**Structure:**
```json
{
  "trophies": ["1", "2", "1", "5"],
  "trophyValue": 35
}
```

**Characteristics:**
- `trophies`: Array of trophy ID strings (duplicates allowed)
- `trophyValue`: Calculated field (may be incorrect)
- Empty arrays exist: 1,284 users have `trophies: []`

**Extreme Cases:**
- Maximum array length: 2,009 (legitimate)
- User with 2,009 trophies has only 3 unique trophy IDs
- User with 1,110 trophies has only 1 unique trophy ID

---

### Settings Object (JSON)

**Structure:**
```json
{
  "dedication_display": 2,
  "stack_roles": 1,
  "hide_unused_trophies": 1,
  "hide_quit_users": 0,
  "leaderboard_format": 0
}
```

**Characteristics:**
- Can be empty object `{}` (1 guild in production)
- Most guilds have partial settings (some keys missing)
- Missing keys should use defaults from `globals.js:52-88`

**Default Values:**
- `dedication_display`: 2 (Mention Only in Server)
- `stack_roles`: 1 (Only Highest Reward)
- `hide_unused_trophies`: 1 (Show Unused)
- `hide_quit_users`: 0 (Hide Quit Users)
- `leaderboard_format`: 0 (Mention)

---

### Rewards Array (JSON)

**Structure:**
```json
[
  {"role": "985813432026681385", "requirement": 5000},
  {"role": "985813409700397106", "requirement": 2500},
  {"role": "985813381950881793", "requirement": 1500}
]
```

**Characteristics:**
- Most guilds have `[]` empty array
- No timestamps in JSON (will add `created_at` in migration)
- Maximum 20 per guild (enforced in code)
- Role IDs are Discord snowflakes (BIGINT)

---

### Panel Object (JSON)

**Structure:**
```json
{
  "message": "993424831842361414",
  "channel": "993424682638381107"
}
```

**Characteristics:**
- Only 1 guild in production has a panel
- Only one panel per guild allowed
- IDs are Discord snowflakes as strings

---

## PostgreSQL Schema Design

### Core Tables

#### 1. guilds
```sql
CREATE TABLE guilds (
    id BIGINT PRIMARY KEY,
    name VARCHAR(100),
    is_safe BOOLEAN DEFAULT false,
    created_at TIMESTAMP DEFAULT NOW(),
    updated_at TIMESTAMP DEFAULT NOW()
);
```

**Migration Notes:**
- `id`: Discord guild ID (direct from JSON key)
- `name`: For reference (not in JSON, fetch from Discord or set later)
- `is_safe`: From `guild.imsafe` field

---

#### 2. trophies
```sql
CREATE TABLE trophies (
    id SERIAL PRIMARY KEY,
    guild_id BIGINT NOT NULL REFERENCES guilds(id) ON DELETE CASCADE,
    legacy_id VARCHAR NOT NULL,
    creator_user_id BIGINT NOT NULL DEFAULT 0,
    created_at TIMESTAMP NOT NULL DEFAULT '1970-01-01 00:00:00',
    name VARCHAR(32) NOT NULL,
    description VARCHAR(128) NOT NULL DEFAULT '',
    emoji VARCHAR(64) NOT NULL DEFAULT '🏆',
    value INTEGER NOT NULL CHECK (value BETWEEN -999999 AND 999999),
    image_filename VARCHAR(255),
    dedication_user_id BIGINT,
    dedication_text VARCHAR(32),
    details VARCHAR(300) NOT NULL DEFAULT 'No details provided.',
    signed BOOLEAN NOT NULL DEFAULT false,

    CONSTRAINT unique_legacy_id UNIQUE(guild_id, legacy_id),
    INDEX idx_trophy_name (guild_id, name),
    INDEX idx_legacy_lookup (guild_id, legacy_id)
);
```

**Migration Notes:**
- `legacy_id`: Original string ID from JSON ("1", "2", "150")
- `creator_user_id`: Use `0` for 43 legacy trophies without creator
- `created_at`: Use epoch `1970-01-01` for legacy trophies
- `image_filename`: Can be filename or full URL
- `dedication_user_id` and `dedication_text`: Both nullable
- `details`: Use default string for missing values

---

#### 3. user_trophies
```sql
CREATE TABLE user_trophies (
    id SERIAL PRIMARY KEY,
    guild_id BIGINT NOT NULL REFERENCES guilds(id) ON DELETE CASCADE,
    user_id BIGINT NOT NULL,
    trophy_id INTEGER NOT NULL REFERENCES trophies(id) ON DELETE CASCADE,
    awarded_by BIGINT,
    awarded_at TIMESTAMP NOT NULL DEFAULT NOW(),

    INDEX idx_user_trophies (guild_id, user_id),
    INDEX idx_trophy_awards (trophy_id),
    INDEX idx_awarded_date (guild_id, awarded_at DESC)
);
```

**CRITICAL:** No UNIQUE constraint - duplicates must be allowed

**Migration Notes:**
- Each element in JSON array = 1 row
- `awarded_by`: NULL for all migrated data (not tracked in JSON)
- `awarded_at`: Use NOW() or incremental timestamps

---

#### 4. guild_settings
```sql
CREATE TABLE guild_settings (
    guild_id BIGINT PRIMARY KEY REFERENCES guilds(id) ON DELETE CASCADE,
    dedication_display SMALLINT NOT NULL DEFAULT 2,
    stack_roles SMALLINT NOT NULL DEFAULT 1,
    hide_unused_trophies SMALLINT NOT NULL DEFAULT 1,
    hide_quit_users SMALLINT NOT NULL DEFAULT 0,
    leaderboard_format SMALLINT NOT NULL DEFAULT 0
);
```

**Migration Notes:**
- Always insert row for each guild (use defaults if JSON is `{}`)
- Parse JSON settings, apply defaults for missing keys

---

#### 5. role_rewards
```sql
CREATE TABLE role_rewards (
    id SERIAL PRIMARY KEY,
    guild_id BIGINT NOT NULL REFERENCES guilds(id) ON DELETE CASCADE,
    role_id BIGINT NOT NULL,
    requirement INTEGER NOT NULL CHECK (requirement >= 1),
    created_at TIMESTAMP NOT NULL DEFAULT NOW(),

    CONSTRAINT unique_guild_role UNIQUE(guild_id, role_id),
    INDEX idx_rewards_lookup (guild_id, requirement DESC)
);
```

**Migration Notes:**
- `created_at`: Use NOW() (not in JSON)
- Order by requirement DESC for doRewardRoles logic

---

#### 6. leaderboard_panels
```sql
CREATE TABLE leaderboard_panels (
    guild_id BIGINT PRIMARY KEY REFERENCES guilds(id) ON DELETE CASCADE,
    channel_id BIGINT NOT NULL,
    message_id BIGINT NOT NULL,
    created_at TIMESTAMP NOT NULL DEFAULT NOW()
);
```

**Migration Notes:**
- Only 1 guild has this in production
- One panel per guild maximum

---

#### 7. bot_stats
```sql
CREATE TABLE bot_stats (
    key VARCHAR(50) PRIMARY KEY,
    value BIGINT NOT NULL DEFAULT 0,
    updated_at TIMESTAMP NOT NULL DEFAULT NOW()
);
```

**Migration Notes:**
- Recalculate all values from actual data
- Keys: `commands.total`, `commands.award`, `commands.create`, etc.
- Do NOT trust bot_db.json values

---

## Critical Business Logic Functions

This section documents Node.js functions with complex logic that must be correctly implemented in Rust.

### 1. getTrophy() - Fuzzy Trophy Search

**Purpose:** Resolve trophy name or ID to actual trophy ID string.

**Algorithm:**
```javascript
// Location: globals.js:121-147
async function getTrophy(client, guild, trophy){
    // Step 1: Try exact ID match if input is numeric
    const isNumber = !Number.isNaN(parseInt(trophy));
    if (isNumber){
        const exists = client.db.guilds.has(`data.${guild}.trophies.${trophy}`);
        if (exists) return trophy;  // Return as string
    }

    // Step 2: Fuzzy search by name
    const name = await parseName(trophy);  // Normalize input
    const trophies = client.db.guilds.get(`data.${guild}.trophies`);

    for (const key in trophies){
        if (key == 'current') continue;

        const trophyName = trophies[key]?.name;
        const checker = await parseName(trophyName);  // Normalize trophy name

        if (checker && checkName(name, checker)){  // Substring match
            return key;  // Return first match
        }
    }

    return null;  // Not found
}

// Helper: parseName (line 237)
function parseName(text){
    if (!text) return text;
    return text.toLowerCase().replace(/\W/g, '').replace(/ /g, '');
    // Removes: non-alphanumeric chars and spaces
    // Example: "My Trophy!" → "mytrophy"
}

// Helper: checkName (line 242)
function checkName(first, second){
    return second.includes(first);  // Substring match
    // Example: checkName("gold", "goldentrophy") → true
}
```

**Behavior:**
1. **Exact ID match** (if numeric): `/award 5 @user` → finds trophy ID "5"
2. **Fuzzy name match**: `/award gold @user` → finds "Golden Trophy" or "Gold Medal"
3. **Case insensitive**: `/award GOLD @user` → same as above
4. **Partial match**: `/award med @user` → finds "Medal" or "Medicinal"
5. **Returns first match** - order depends on JSON iteration (unreliable)

**Used in:**
- `award.js:25` - Award trophy to user
- `revoke.js:25` - Revoke trophy from user
- `delete.js:22` - Delete trophy
- `edit.js:26` - Edit trophy
- `details.js:20` - Show trophy details
- `show.js:20` - Display trophy

**Migration Notes:**
- Always returns **string** (even for numeric IDs)
- Fuzzy search is **substring match** after normalization
- No error on multiple matches - returns first found
- Must implement same normalization in Rust:
  ```rust
  fn parse_name(text: &str) -> String {
      text.to_lowercase()
          .chars()
          .filter(|c| c.is_alphanumeric())
          .collect()
  }

  fn check_name(input: &str, trophy_name: &str) -> bool {
      trophy_name.contains(input)
  }
  ```

---

### 2. doRewardRoles() - Automatic Role Assignment

**Purpose:** Assign/remove Discord roles based on user's trophy score.

**Algorithm:**
```javascript
// Location: globals.js:169-235
async function doRewardRoles(client, guild, id){
    // Step 1: Check bot has MANAGE_ROLES permission
    const manageRoles = guild.me.permissions.has('MANAGE_ROLES');
    if (!manageRoles) return;  // Silent fail

    // Step 2: Get user score and rewards
    const rewards = client.db.guilds.get(`data.${guild.id}.rewards`);
    if (!rewards || !rewards.length) return;

    const user = client.db.guilds.get(`data.${guild.id}.users.${id}`);
    if (!user) return;

    const score = user.trophyValue;  // Uses stored value (may be wrong!)

    // Step 3: Get stack_roles setting
    const config = client.db.guilds.get(`data.${guild.id}.settings`);
    const stackRoles = config?.stack_roles ?? 1;
    // 0 = Stack all roles, 1 = Only highest role

    // Step 4: Calculate which roles to add/remove
    let award = [], remove = [], prevRole = null;
    let foundBest = false;

    for (const reward of rewards){  // Assumes sorted by requirement DESC
        if (score < reward.requirement){
            // User doesn't qualify for this role
            if (prevRole != null) remove.push(prevRole);
            prevRole = reward.role;
        } else {
            // User qualifies for this role
            if (!foundBest){
                // First qualifying role (highest)
                award.push(reward.role);
                if (prevRole != null) remove.push(prevRole);
                foundBest = true;
                continue;
            }

            // Additional qualifying roles
            if (stackRoles == 0)
                award.push(reward.role);  // Stack mode: add all
            else
                remove.push(reward.role);  // Only highest: remove lower

            foundBest = true;
        }
    }

    if (!foundBest) remove.push(prevRole);

    // Step 5: Apply changes
    award = award.filter(x => x);    // Remove nulls
    remove = remove.filter(x => x);  // Remove nulls

    const member = await guild.members.fetch(id);
    await member.roles.add(award, `Role rewards`);
    await member.roles.remove(remove, `Role rewards`);
}
```

**Behavior Examples:**

**Example 1: stack_roles = 1 (Only Highest)**
- Rewards: 5000pts, 2500pts, 1000pts, 500pts
- User has 2800pts
- Result: Gets 2500pts role only, removes 1000pts and 500pts

**Example 2: stack_roles = 0 (Stack All)**
- Rewards: 5000pts, 2500pts, 1000pts, 500pts
- User has 2800pts
- Result: Gets 2500pts + 1000pts + 500pts roles

**Called from:**
- `award.js:72` - After awarding trophies
- `revoke.js:72` - After revoking trophies
- `clear.js:25` - After clearing all trophies

**Migration Notes:**
- Requires **MANAGE_ROLES** permission (silent fail if missing)
- Assumes rewards array sorted by requirement DESC
- Uses `trophyValue` directly (may be desynchronized)
- In Rust: Calculate score from `SUM(trophies.value)` first
- Role hierarchy: Bot cannot assign roles above its own position
- Silent failures on permission errors

**Rust Implementation Strategy:**
```rust
async fn do_reward_roles(
    ctx: &Context,
    guild_id: GuildId,
    user_id: UserId,
    score: i64,  // Pre-calculated from DB
    stack_roles: i16
) -> Result<()> {
    // 1. Check bot permissions
    // 2. Fetch rewards sorted by requirement DESC
    // 3. Calculate award/remove lists
    // 4. Apply role changes
}
```

---

### 3. cleanseTrophies() - Delete Trophy from All Users

**Purpose:** Remove all instances of a trophy from all users when trophy is deleted.

**Algorithm:**
```javascript
// Location: globals.js:252-265
async function cleanseTrophies(client, guild, trophy, value){
    const users = client.db.guilds.get(`data.${guild}.users`);

    for (const id in users){
        if (!users[id].trophies) continue;

        // Remove ALL occurrences of this trophy
        while (users[id].trophies.includes(trophy)){
            users[id].trophies.splice(users[id].trophies.indexOf(trophy), 1);
            users[id].trophyValue -= value;  // Manual decrement
        }
    }

    client.db.guilds.set(`data.${guild}.users`, users);
}
```

**Behavior:**
- Removes **all occurrences** of trophy from every user
- Manually decrements `trophyValue` (can cause desync if value is wrong)
- No error handling - silently continues on errors
- Called from `delete.js:45` before deleting trophy

**PostgreSQL Equivalent:**
```sql
-- Much simpler with foreign keys
DELETE FROM user_trophies
WHERE trophy_id = $1;
-- ON DELETE CASCADE handles this automatically
```

**Migration Notes:**
- PostgreSQL `ON DELETE CASCADE` replaces this function
- No need to manually update scores (calculated from remaining trophies)
- More reliable and atomic
- No risk of partial deletions

---

## Migration Algorithm

### Phase 0: Preparation

```rust
// Load production data directly from json.sqlite
let legacy = LegacyData::load("sqlite://json.sqlite").await?;
let guilds_data = legacy.guilds(); // HashMap<String, GuildData>
let bot_data = legacy.bot();       // BotData struct with command counters

// Create PostgreSQL connection with transaction
let mut tx = pool.begin().await?;

// Create mapping storage
let mut trophy_id_mappings: HashMap<(String, String), i64> = HashMap::new();
// Key: (guild_id, legacy_id), Value: new_id
```

**Why LegacyData?**
- Guarantees we read the exact blobs the Node bot uses today (no stale JSON snapshots)
- Centralizes decompression/parsing logic so the migration and Rust bot stay in sync
- Works for both SQLite (dev) and PostgreSQL (prod) through the same serde structs

---

### Phase 1: Migrate Guilds

```rust
use entities::guilds;
use sea_orm::{ActiveModelTrait, Set};

for (guild_id_str, guild_data) in guilds_data.iter() {
    let guild_id: i64 = guild_id_str.parse()?;

    guilds::ActiveModel {
        id: Set(guild_id),
        is_safe: Set(guild_data.imsafe.unwrap_or(false)),
        ..Default::default()
    }
    .insert(&tx)
    .await?;

    migrated_guilds += 1;
}
```

**Expected:** 2,493 guilds

---

### Phase 2: Migrate Trophies with ID Mapping

```rust
use entities::trophies;

for (guild_id_str, guild_data) in guilds_data.iter() {
    let guild_id: i64 = guild_id_str.parse()?;

    if let Some(trophies_map) = &guild_data.trophies {
        for (legacy_id, trophy) in trophies_map {
            if legacy_id == "current" {
                continue;
            }

            let creator = trophy
                .creator
                .as_ref()
                .and_then(|s| s.parse::<i64>().ok())
                .unwrap_or(0);

            let created_ts = trophy
                .created
                .and_then(|ms| DateTime::from_timestamp(ms / 1000, 0))
                .unwrap_or(DateTime::UNIX_EPOCH);

            let details = trophy
                .details
                .as_deref()
                .unwrap_or("No details provided.");

            let signed = trophy.signed.unwrap_or(false);
            let (dedication_user, dedication_text) = parse_dedication(&trophy.dedication);

            let inserted = trophies::ActiveModel {
                guild_id: Set(guild_id),
                legacy_id: Set(legacy_id.clone()),
                creator_user_id: Set(creator),
                created_at: Set(created_ts),
                name: Set(trophy.name.clone()),
                description: Set(trophy.description.clone()),
                emoji: Set(trophy.emoji.clone()),
                value: Set(trophy.value),
                image_filename: Set(trophy.image.clone()),
                dedication_user_id: Set(dedication_user),
                dedication_text: Set(dedication_text),
                details: Set(details.to_string()),
                signed: Set(signed),
                ..Default::default()
            }
            .insert(&tx)
            .await?;

            let new_id = inserted.id.unwrap();
            trophy_id_mappings.insert((guild_id_str.clone(), legacy_id.clone()), new_id);
            migrated_trophies += 1;
        }
    }
}
```

**Expected:** 10,853 trophies
**Legacy trophies (defaults needed):** 43

> **SeaORM migrations remain the source of truth.**  
> Each column/index described above must be mirrored in `src/migrations` via the SeaORM migrator so `cargo run -- migrate up` produces the exact schema documented here.

---

### Phase 3: Migrate User Trophy Awards

```rust
use chrono::{DateTime, TimeZone, Utc};
use entities::user_trophies;

let mut synthetic_award_clock: i64 = 1_600_000_000; // Fixed base keeps inserts deterministic

for (guild_id_str, guild_data) in guilds_data.iter() {
    let guild_id: i64 = guild_id_str.parse()?;

    if let Some(users) = &guild_data.users {
        for (user_id_str, user_data) in users {
            let user_id: i64 = user_id_str.parse()?;

            if let Some(trophy_array) = &user_data.trophies {
                for legacy_trophy_id in trophy_array {
                    let key = (guild_id_str.clone(), legacy_trophy_id.clone());

                    if let Some(&new_trophy_id) = trophy_id_mappings.get(&key) {
                        let awarded_at = Utc
                            .timestamp_opt(synthetic_award_clock, 0)
                            .single()
                            .unwrap_or(DateTime::UNIX_EPOCH);
                        synthetic_award_clock += 1; // Maintain stable ordering per insert

                        user_trophies::ActiveModel {
                            guild_id: Set(guild_id),
                            user_id: Set(user_id),
                            trophy_id: Set(new_trophy_id),
                            awarded_by: Set(None),
                            awarded_at: Set(awarded_at),
                            ..Default::default()
                        }
                        .insert(&tx)
                        .await?;

                        migrated_awards += 1;
                    } else {
                        warnings.push(format!(
                            "Orphaned trophy: guild={}, user={}, legacy_id={}",
                            guild_id, user_id, legacy_trophy_id
                        ));
                        orphaned_awards += 1;
                    }
                }
            }
        }
    }
}
```

**Expected:** 60,554 awards
**Orphaned (trophy doesn't exist):** Should be 0
**Synthetic timestamps:** Quick.db never stored award times, so we advance a deterministic counter (base UNIX timestamp + 1 second per insert) to keep ordering reproducible across dry-runs.

---

### Phase 4: Migrate Guild Settings

```rust
use entities::guild_settings;

for (guild_id_str, guild_data) in guilds_data.iter() {
    let guild_id: i64 = guild_id_str.parse()?;
    let config = guild_data
        .settings
        .as_ref()
        .cloned()
        .unwrap_or_default(); // serde struct mirrors JSON keys

    guild_settings::ActiveModel {
        guild_id: Set(guild_id),
        dedication_display: Set(config.dedication_display.unwrap_or(2) as i16),
        stack_roles: Set(config.stack_roles.unwrap_or(1) as i16),
        hide_unused_trophies: Set(config.hide_unused_trophies.unwrap_or(1) as i16),
        hide_quit_users: Set(config.hide_quit_users.unwrap_or(0) as i16),
        leaderboard_format: Set(config.leaderboard_format.unwrap_or(0) as i16),
        ..Default::default()
    }
    .insert(&tx)
    .await?;

    migrated_settings += 1;
}
```

**Expected:** 2,493 settings rows

---

### Phase 5: Migrate Role Rewards

```rust
use entities::role_rewards;

for (guild_id_str, guild_data) in guilds_data.iter() {
    let guild_id: i64 = guild_id_str.parse()?;

    if let Some(rewards) = &guild_data.rewards {
        for reward in rewards {
            role_rewards::ActiveModel {
                guild_id: Set(guild_id),
                role_id: Set(reward.role.parse()?),
                requirement: Set(reward.requirement),
                ..Default::default()
            }
            .insert(&tx)
            .await?;

            migrated_rewards += 1;
        }
    }
}
```

**Expected:** ~5,000 rewards

---

### Phase 6: Migrate Leaderboard Panels

```rust
use entities::leaderboard_panels;

for (guild_id_str, guild_data) in guilds_data.iter() {
    let guild_id: i64 = guild_id_str.parse()?;

    if let Some(panel) = &guild_data.panel {
        leaderboard_panels::ActiveModel {
            guild_id: Set(guild_id),
            channel_id: Set(panel.channel.parse()?),
            message_id: Set(panel.message.parse()?),
            ..Default::default()
        }
        .insert(&tx)
        .await?;

        migrated_panels += 1;
    }
}
```

**Expected:** 1 panel

---

### Phase 7: Recalculate Bot Stats

```rust
use entities::{bot_stats, trophies, user_trophies};
use sea_orm::{ConnectionTrait, EntityTrait, QuerySelect};

// Recalculate from actual data (don't trust bot_db.json)
let trophy_count = trophies::Entity::find().count(&tx).await?;
insert_stat(&tx, "trophies.total", trophy_count).await?;

let award_count = user_trophies::Entity::find().count(&tx).await?;
insert_stat(&tx, "awards.total", award_count).await?;

for (command, count) in &bot_data.commands {
    let key = format!("commands.{}", command);
    insert_stat(&tx, &key, *count as i64).await?;
}

async fn insert_stat<C: ConnectionTrait>(conn: &C, key: &str, value: i64) -> Result<(), DbErr> {
    bot_stats::ActiveModel {
        key: Set(key.to_string()),
        value: Set(value),
        ..Default::default()
    }
    .insert(conn)
    .await?;
    Ok(())
}
```

---

### Phase 8: Commit Transaction

```rust
// Final validation before commit
assert_eq!(migrated_guilds, 2493, "Guild count mismatch");
assert_eq!(migrated_trophies, 10853, "Trophy count mismatch");
assert_eq!(migrated_awards, 60554, "Award count mismatch");
assert_eq!(orphaned_awards, 0, "Found orphaned awards");

// Commit all changes
tx.commit().await?;

println!("✅ Migration completed successfully");
println!("   Guilds: {}", migrated_guilds);
println!("   Trophies: {}", migrated_trophies);
println!("   Awards: {}", migrated_awards);
println!("   Settings: {}", migrated_guilds);
println!("   Rewards: {}", migrated_rewards);
println!("   Panels: {}", migrated_panels);
```

---

## Validation Strategy

### Pre-Migration Validation

```bash
# 1. Backup everything
cp json.sqlite json.sqlite.backup.$(date +%Y%m%d_%H%M%S)
tar -czf images_backup.tar.gz ./images/

# 2. Verify JSON integrity
node -e "require('./guilds_db.json')" # Should not error
node -e "require('./bot_db.json')"   # Should not error

# 3. Count records
node -e "
  const data = require('./guilds_db.json');
  console.log('Guilds:', Object.keys(data).length);
"
```

---

### Post-Migration Validation

#### 1. Count Validation
```sql
-- Guilds
SELECT COUNT(*) FROM guilds; -- Expected: 2,493

-- Trophies
SELECT COUNT(*) FROM trophies; -- Expected: 10,853

-- Trophy Awards
SELECT COUNT(*) FROM user_trophies; -- Expected: 60,554

-- Settings
SELECT COUNT(*) FROM guild_settings; -- Expected: 2,493

-- Check for orphaned data
SELECT COUNT(*) FROM user_trophies ut
LEFT JOIN trophies t ON ut.trophy_id = t.id
WHERE t.id IS NULL; -- Expected: 0
```

---

#### 2. Score Validation (Sample)

```rust
// Validate scores for random sample of 100 users
let sample_users = select_random_users(&guilds_data, 100);

// Pre-compute legacy trophy values for quick lookup
let mut legacy_values: HashMap<(String, String), i64> = HashMap::new();
for (guild_id, guild) in guilds_data.iter() {
    if let Some(trophies) = &guild.trophies {
        for (legacy_id, trophy) in trophies {
            legacy_values.insert((guild_id.clone(), legacy_id.clone()), trophy.value.into());
        }
    }
}

for (guild_id, user_id) in sample_users {
    // Calculate from JSON: sum trophy.value for every legacy ID in the array
    let json_awards = guilds_data[&guild_id]
        .users
        .as_ref()
        .and_then(|users| users.get(&user_id))
        .and_then(|user| user.trophies.as_ref())
        .cloned()
        .unwrap_or_default();

    let json_score: i64 = json_awards
        .iter()
        .map(|legacy_id| {
            legacy_values
                .get(&(guild_id.clone(), legacy_id.clone()))
                .copied()
                .unwrap_or(0)
        })
        .sum();

    // Calculate from DB
    let db_score: i64 = sqlx::query_scalar!(
        r#"
        SELECT COALESCE(SUM(t.value), 0)
        FROM user_trophies ut
        JOIN trophies t ON ut.trophy_id = t.id
        WHERE ut.guild_id = $1 AND ut.user_id = $2
        "#,
        guild_id,
        user_id
    )
    .fetch_one(&pool)
    .await?;

    if json_score != db_score {
        println!(
            "⚠️ Score mismatch: guild={}, user={}, JSON={}, DB={}",
            guild_id, user_id, json_score, db_score
        );
        score_mismatches += 1;
    }
}

println!(
    "Score validation: {}/{} matched",
    100 - score_mismatches,
    100
);
```

**Expected:** All mismatches indicate a true migration bug (since both sides now sum values from the same legacy data)

---

#### 3. Duplicate Validation

```sql
-- Verify duplicates were preserved
SELECT
    guild_id,
    user_id,
    trophy_id,
    COUNT(*) as count
FROM user_trophies
GROUP BY guild_id, user_id, trophy_id
HAVING COUNT(*) > 1
ORDER BY count DESC
LIMIT 10;
```

**Expected:** Many rows with count > 1

---

#### 4. Legacy Trophy Validation

```sql
-- Check legacy trophies got defaults
SELECT
    COUNT(*) as legacy_count
FROM trophies
WHERE creator_user_id = 0
   OR created_at = '1970-01-01 00:00:00'
   OR details = 'No details provided.';
```

**Expected:** 43 legacy trophies

---

#### 5. Image File Validation

```rust
// Verify all referenced images exist
let images_in_db: Vec<String> = sqlx::query_scalar!(
    "SELECT image_filename FROM trophies WHERE image_filename IS NOT NULL"
).fetch_all(&pool).await?;

for image in images_in_db {
    if image.starts_with("http") {
        // External URL - skip validation
        continue;
    }

    let path = format!("./images/{}", image);
    if !Path::new(&path).exists() {
        println!("⚠️ Missing image file: {}", path);
        missing_images += 1;
    }
}

println!("Image validation: {} missing files", missing_images);
```

---

#### 6. SeaORM Migration Verification

```bash
cargo run -- migrate status
```

- Should report **Applied** for every migration that encodes the schema described in this guide.
- If any pending migration exists, stop and reconcile `src/migrations` with MIGRATION.md before running the import.

## Implementation Decisions

### Decision Log

| # | Issue | Decision | Rationale |
|---|-------|----------|-----------|
| 1 | Bug in revoke.js pop() | **FIX** - Implement correct behavior | Bug causes wrong trophies to be removed |
| 2 | Bot counters desync | **IGNORE** - Recalculate all | Counters are historically cumulative, not current state |
| 3 | trophyValue storage | **DO NOT MIGRATE** - Always calculate | Prevents desynchronization, single source of truth |
| 4 | Legacy trophy fields | **USE DEFAULTS** - 0/epoch/false | Cannot recover missing data from old versions |
| 5 | awarded_by field | **USE NULL** - No data available | JSON doesn't track who awarded trophies |
| 6 | awarded_at timestamps | **SYNTHETIC ORDER** - Increment counter | Quick.db lacks timestamps; deterministic synthetic clock keeps inserts reproducible |
| 7 | Duplicate awards | **PRESERVE** - Required functionality | Core feature of the system |
| 8 | Trophy ID type | **STRING → INTEGER** - With legacy_id column | Modern auto-increment, keep mapping for reference |
| 9 | Permissions system | **DO NOT MIGRATE** - Deprecated since v1.4 | No longer used in production |
| 10 | Score mismatches | **USE DB CALCULATION** - Trust recalculated | JSON values may be incorrect |

---

### Rollback Plan

**If migration fails or critical issues found:**

1. **Stop new Rust bot** immediately
2. **Restore Node.js bot** from backup
3. **Restore json.sqlite** from backup
4. **Verify Node.js bot functional**
5. **Analyze failure** and fix migration script
6. **Retry migration** after fixes

**Rollback window:** 24 hours after successful migration

**Rollback triggers:**
- Data loss detected
- Critical functionality broken
- Performance degradation > 10x
- User complaints > 10 in first hour

---

### Testing Strategy

**Before Production Migration:**

1. **Test migration with subset** (100 guilds)
2. **Validate all counts and scores**
3. **Test Rust bot with migrated data**
4. **Performance testing** (leaderboard, search, award)
5. **Full migration dry-run** on production copy

**Production Migration:**

1. **Announce maintenance window** (30-60 min)
2. **Final backup** of all data
3. **Execute migration script**
4. **Run all validation queries**
5. **Deploy Rust bot**
6. **Test critical commands** (/award, /leaderboard, /create)
7. **Monitor errors** for first 24 hours

---

## Migration Checklist

### Pre-Migration
- [ ] Backup `json.sqlite`
- [ ] Backup `./images/` directory
- [ ] Backup `guilds_db.json` and `bot_db.json` separately
- [ ] Ensure `cargo run -- migrate status` shows no pending SeaORM migrations
- [ ] Test migration script on copy (100 guilds)
- [ ] Test migration script on copy (all data)
- [ ] Verify PostgreSQL schema created
- [ ] Verify Rust bot compiles and runs
- [ ] Prepare rollback script
- [ ] Announce maintenance window

### Migration
- [ ] Stop Node.js bot
- [ ] Run migration script
- [ ] Verify guild count (2,493)
- [ ] Verify trophy count (10,853)
- [ ] Verify award count (60,554)
- [ ] Verify no orphaned records
- [ ] Sample score validation (100 users)
- [ ] Verify image files referenced
- [ ] Verify duplicates preserved

### Post-Migration
- [ ] Deploy Rust bot
- [ ] Test `/award` command
- [ ] Test `/create` command
- [ ] Test `/leaderboard` command
- [ ] Test `/trophies` command
- [ ] Monitor error logs (first hour)
- [ ] Check Discord API rate limits
- [ ] Verify role rewards working
- [ ] Test with high-traffic guild
- [ ] Announce successful migration

### 24-Hour Monitoring
- [ ] Check error rates
- [ ] Monitor command response times
- [ ] Review user feedback
- [ ] Verify data consistency
- [ ] Archive Node.js backup
- [ ] Document any issues found

---

## Expected Outcomes

### Performance Improvements
- **Leaderboard queries:** 50-100x faster (indexed SQL vs JSON parsing)
- **Trophy search:** 10-20x faster (indexed name search)
- **Award operations:** Similar speed (simple inserts)
- **Score calculation:** Always accurate (no desync possible)

### Data Improvements
- **Accurate counters:** Recalculated from source
- **Consistent scores:** Always calculated from trophies
- **No lost data:** All 60,554 awards preserved
- **Duplicates working:** Proper table structure

### Bug Fixes
- **revoke.js bug:** Fixed - removes correct trophies
- **Counter desync:** Fixed - real-time calculation
- **Score desync:** Fixed - calculated from source

---

## Contact & Support

**Migration Lead:** Development Team
**Documentation Version:** 1.0
**Last Updated:** 2025-10-15

**For issues during migration:**
1. Stop migration immediately
2. Do NOT commit transaction
3. Restore from backup
4. Document error details
5. Contact development team

---

**End of Migration Guide**

## Resume

Objetivo Global

- Migrar el bot de Node.js/Discord.js + quick.db (dos blobs JSON con toda la información) a un backend en Rust con Serenity/Poise y SeaORM, manteniendo todas las funcionalidades pero corrigiendo errores
  históricos.
- Quick.db guarda todo en bot_db.json y guilds_db.json mediante rutas como data.<guild>.trophies.<id> (CLAUDE.md:24-64, COMMANDS_AND_FUNCTIONALITY.md:5-58). Esta representación impide consultas fiables,
  duplicó errores de contadores y dejó bugs sin corregir.
- El nuevo stack usa migraciones SeaORM (src/migrations/mod.rs:1-64) y entidades con relaciones reales: guilds, trophies, user_trophies, guild_settings, role_rewards, leaderboard_panels, bot_stats
  (MIGRATION.md:368-516). SQLite se usa para desarrollo y PostgreSQL en producción; SeaORM abstrae ambos.

Por qué migrar (datos y bugs)

- Estadísticas reales: 2493 guilds, 10853 trofeos, 60554 premios, 8299 usuarios con trofeos (MIGRATION.md:42-54).
- Contadores globales están desincronizados por no decrementar al revocar/limpiar (MIGRATION.md:55-116).
- revoke.js usa array.pop(id) y elimina elementos incorrectos; trophyValue se recalcula manualmente y desincroniza; los IDs de trofeo son strings y pueden duplicarse en arrays de usuarios; 43 trofeos carecen
  de metadatos (MIGRATION.md:78-214).
- Reparaciones exigidas: ID mapping legacy_id→id, duplicados permitidos en user_trophies, recalcular puntuaciones con SUM(t.value), default para trofeos incompletos, panel/reward/settings intactos.

Base de datos nueva (SeaORM)

- guilds: clave primaria id (snowflake), is_safe (de imsafe), timestamps (MIGRATION.md:372-388).
- trophies: id SERIAL, guild_id FK, legacy_id, creator_user_id, created_at, name, description, emoji, value, image_filename, dedication_user_id/text, details, signed, índices para legacy_id y nombre
  (MIGRATION.md:390-421).
- user_trophies: guild_id, user_id, trophy_id, awarded_at, awarded_by (NULL en migración); sin UNIQUE para permitir duplicados (MIGRATION.md:424-446).
- guild_settings: fila por guild con defaults (dedication_display=2, etc., MIGRATION.md:449-459).
- role_rewards y leaderboard_panels: reflejan arreglos JSON (MIGRATION.md:467-516).
- bot_stats: claves commands.*, trophies.total, etc., recalculadas tras la importación (MIGRATION.md:503-516).

Lectura del legado

- src/legacy/mod.rs:1-43 ya implementa LegacyData::load(): abre sqlite://json.sqlite, extrae la columna json de las tablas bot/guilds y expone serde_json para usarlas en el importador. Esta capa permite
  trabajar con los mismos datos que usa Node sin depender de los archivos plano.

Algoritmo paso a paso (SeaORM)

1. Preparación
  - Cargar LegacyData (legacy::LegacyData::load()) y mapear a structs específicos (guild_id, users, trophies, etc.).
  - Abrir DatabaseConnection con SeaORM y comenzar DatabaseTransaction.
  - Preparar HashMap<(guild_id, legacy_id), i64> para mapear IDs de trofeos, contadores (migrated_*), lista de advertencias y acumuladores (MIGRATION.md:762-781).
2. Insertar guilds
  - Recorrer cada entrada de guilds_db.json.
  - Convertir el ID (string) a i64.
  - Insertar fila en guilds usando ActiveModel (GuildActiveModel { id: Set(guild_id), is_safe: Set(guild_data.imsafe.unwrap_or(false)), .. }).
  - Guardar migrated_guilds y validar que llegue a 2493 (MIGRATION.md:783-803).
3. Migrar trofeos + mapping
  - Para cada guild, iterar guild_data.trophies excluyendo la clave "current".
  - Para los 43 trofeos con campos faltantes, aplicar defaults: creator=0, created_at=UNIX_EPOCH, signed=false, details="No details provided." (ver estadísticas en MIGRATION.md:118-214).
  - Parsear dedicación (dedication.user/name) a dedication_user_id y dedication_text.
  - Insertar usando SeaORM y recuperar id nuevo (insert().await? devuelve InsertResult).
  - Guardar en el HashMap (guild_id, legacy_id) el nuevo id.
  - Contar migrated_trophies, esperar 10853.
4. Migrar premios por usuario
  - Recorrer guild_data.users. Cada usuario tiene trophies: Vec<String> y trophyValue (ignorarlo).
  - Por cada string, buscar (guild_id, legacy_id) en el mapa.
  - Insertar user_trophies::ActiveModel con guild_id, user_id, trophy_id y awarded_at (puede ser NOW() o timestamps derivados si se desea; MIGRATION.md:424-446).
  - Permitir duplicados por usuario (no hay UNIQUE).
  - Si falta mapping (no debería), registrar advertencia y contar orphaned_awards.
  - Objetivo: 60554 filas y 0 huérfanos (MIGRATION.md:868-912).
5. Insertar settings
  - Para cada guild, obtener settings (puede ser {}) y aplicar defaults documentados (dedication_display=2, stack_roles=1, etc., MIGRATION.md:303-349).
  - Insertar una fila en guild_settings con esos valores.
6. Insertar role rewards
  - Recorrer guild_data.rewards (array).
  - Cada entrada {"role": "id", "requirement": number} se convierte a RoleRewardActiveModel.
  - Insertar (SeaORM), contar migrated_rewards (~5000).
  - El orden en la tabla puede guardarse con created_at, pero la lógica doRewardRoles asume orden descendente por requirement, por lo que las consultas deberán usar ORDER BY requirement DESC.
7. Insertar panels
  - Sólo si guild_data.panel existe, convertir channel y message a i64 y crear fila en leaderboard_panels.
  - Esperado: 1 fila (MIGRATION.md:997-1023).
8. Recalcular estadísticas
  - Usar SeaORM para contar trophies y user_trophies y guardarlos en bot_stats (key = "trophies.total" y "awards.total").
  - Para contadores por comando, bot_db.json.commands sigue siendo útil; insertar cada par ("commands.<name>", count) en bot_stats.
  - Ignorar bot_db.json.trophies y trophiesAwarded por estar corruptos (MIGRATION.md:44-76).
9. Validación antes de commit
  - Comparar migrated_guilds == 2_493, migrated_trophies == 10_853, migrated_awards == 60_554, orphaned_awards == 0 (MIGRATION.md:1054-1073).
  - Consultas SQL/SeaORM para confirmar conteos: Guild::find().count().await?, UserTrophies::find().count().await?, etc.
  - Verificar que no existan premios huérfanos (UserTrophies::find().left_join(Trophies).filter(Trophies::Column::Id.is_null())).
  - Validar muestras de puntuaciones: seleccionar usuarios aleatorios, sumar valores en SQL y comparar con trophies.len()/value del JSON para detectar cualquier error de mapping (MIGRATION.md:1123-1140).
10. Commit y despliegue

- Si todo coincide, confirmar la transacción.
- Preparar el corte: MIGRATION.md:30-39 recomienda un downtime de 30‑60 minutos con rollback al bot Node durante 24h si surge algún fallo. Seguridad: respaldar json.sqlite e imágenes antes
  (MIGRATION.md:1077-1095).

Integración con el bot Rust

- Una vez pobladas las tablas, los comandos descritos en DISCORD_COMMANDS_DOCUMENTATION.md se reimplementan sobre SeaORM: cada handler en Poise (src/bot/commands.rs) consultará/actualizará las entidades
  nuevas (por ejemplo, award insertará en user_trophies y recalculará roles via queries).
- El CLI (src/migrations/mod.rs) ya soporta up, down, fresh, etc. Cuando agregues las migraciones reales, se aplicarán con cargo run -- migrate up.
- LegacyData puede usarse en un binario separado o un subcomando cargo run --bin trophy-bot -- migrate import-json, que ejecute todas las fases previas dentro de la transacción SeaORM.

Conclusión
La migración es un pipeline bien definido: leer quick.db desde SQLite (LegacyData), poblar una base normalizada (SeaORM), arreglar bugs históricos mediante nuevos invariantes (ID mapping, duplicados
permitidos, recálculo de puntuaciones), validar todo y cortar a la versión Rust. Cada paso y su justificación están respaldados en MIGRATION.md; lo que falta es escribir las migraciones reales, los modelos
SeaORM y el importador que siga estas fases exactamente.
