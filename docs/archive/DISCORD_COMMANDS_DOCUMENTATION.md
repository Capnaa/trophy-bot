# Discord Slash Commands Documentation for Trophy Bot - Rust Rewrite Reference

This document provides a comprehensive list of ALL Discord slash commands that must be implemented in the Rust rewrite of Trophy Bot. Each command is documented with exact parameters, permissions, validation rules, business logic, and implementation details.

## Command Structure Overview

All commands follow this basic structure:
- `data`: SlashCommandBuilder configuration
- `permissions`: Custom permission array (deprecated but needs handling)
- `cooldown`: Time in seconds between command uses (optional)
- `run`: Async function that executes the command

## Bot Utility Commands (10 commands)

### 1. `/about`
**Description:** Who am I? Who are you? Questions never asked.

**SlashCommandBuilder:**
```rust
SlashCommandBuilder::new()
    .name("about")
    .description("Who am I? Who are you? Questions never asked.")
```

**Parameters:** None

**Permissions:** None required

**Functionality:**
- Creates an embed with bot information
- Shows GitHub link, Ko-fi donation link, Support Server invite
- Sets thumbnail to bot's avatar
- Uses main color (`#0096FF`)
- Mentions bot creator (@Antikore#9357)

**Response Format:**
- Embed with title "About Trophy Bot 🏆"
- Description with links and basic info
- Bot avatar as thumbnail

**Implementation Notes:**
- Simple informational command
- No database operations
- Static content response

---

### 2. `/forgetme`
**Description:** Remove all images and data about your server from the bot and kick it.

**SlashCommandBuilder:**
```rust
SlashCommandBuilder::new()
    .name("forgetme")
    .description("Remove all images and data about your server from the bot and kick it.")
    .default_member_permissions(Permissions::ADMINISTRATOR)
```

**Parameters:** None

**Permissions:** 
- Discord: Administrator permission (setDefaultMemberPermissions("8"))
- Additional: Only server owner can execute

**Validation:**
- Must check if interaction.guild.ownerId == interaction.member.id
- If not owner, delete reply and return

**Functionality:**
- Shows warning embed about data deletion
- Creates action row with confirmation button
- Button ID: `forgetmeproceed`
- Button style: Danger, emoji: 🧹

**Response Format:**
- Warning embed with error color (`#E02D44`)
- Confirmation button for proceeding
- Bot avatar as thumbnail

**Special Behaviors:**
- Only server owners can use this command
- Requires button interaction to proceed
- Irreversible action warning

---

### 3. `/help`
**Description:** Stop it! Get some help!

**SlashCommandBuilder:**
```rust
SlashCommandBuilder::new()
    .name("help")
    .description("Stop it! Get some help!")
```

**Parameters:** None

**Permissions:** None required

**Functionality:**
- Shows comprehensive command usage guide
- Lists main commands with syntax
- Explains permission system
- Static informational content

**Response Format:**
- Embed with title "How to trophies 101"
- Main color (`#0096FF`)
- Detailed command list in description

**Implementation Notes:**
- No database operations required
- Educational content about bot usage

---

### 4. `/imsafe`
**Description:** Confirms you're using discord permissions instead of the deprecated custom permissions

**SlashCommandBuilder:**
```rust
SlashCommandBuilder::new()
    .name("imsafe")
    .description("Confirms you're using discord permissions instead of the deprecated custom permissions")
    .default_member_permissions(Permissions::MANAGE_GUILD)
```

**Parameters:** None

**Permissions:** Manage Guild permission (setDefaultMemberPermissions("32"))

**Database Operations:**
- Check: `data.${guild}.imsafe` (boolean)
- Set: `data.${guild}.imsafe` = true

**Functionality:**
- Checks if guild is already in safe mode
- If already safe, shows confirmation message
- If not safe, sets safe mode and shows success

**Response Format:**
- Main color embed
- Success message with checkmark emoji
- Different messages for already safe vs newly set

**Implementation Notes:**
- Critical for permission system migration
- Required for management commands to work

---

### 5. `/invite`
**Description:** Invite the bot to your server!

**SlashCommandBuilder:**
```rust
SlashCommandBuilder::new()
    .name("invite")
    .description("Invite the bot to your server!")
```

**Parameters:** None

**Permissions:** None required

**Functionality:**
- Shows bot invite link
- Hardcoded OAuth2 URL with specific permissions
- Ephemeral response (only visible to user)

**Response Format:**
- Main color embed
- Title: "Invite Me to Your Server!"
- Bot avatar as thumbnail
- Ephemeral: true

**Implementation Notes:**
- Static invite URL: `https://discord.com/oauth2/authorize?client_id=985134052665356299&permissions=34816&scope=applications.commands%20bot`
- No database operations

---

### 6. `/language` (DEPRECATED/COMMENTED OUT)
**Status:** Completely commented out in source code
**Implementation:** Should not be implemented in Rust rewrite

---

### 7. `/ping`
**Description:** Current bot ping! If the bot doesn't answer then ping is probably over 5000ms and very likely down

**SlashCommandBuilder:**
```rust
SlashCommandBuilder::new()
    .name("ping")
    .description("Current bot ping! If the bot doesn't answer then ping is probably over 5000ms and very likely down")
```

**Parameters:** None

**Permissions:** None required

**Functionality:**
- Calculates WebSocket ping (interaction.client.ws.ping)
- Calculates bot latency (response time - interaction time)
- Shows both metrics in embed

**Response Format:**
- Main color embed
- Shows bot latency and Discord API ping
- Intermediate "Pinging..." message before final result

**Implementation Notes:**
- Requires timing calculations
- Two-step response process

---

### 8. `/stats`
**Description:** Look at the bot stats

**SlashCommandBuilder:**
```rust
SlashCommandBuilder::new()
    .name("stats")
    .description("Look at the bot stats")
```

**Parameters:** None

**Permissions:** None required

**Cooldown:** 10 seconds

**Database Reads:**
- `data.commands.total` (total command runs)
- `data.trophies` (total trophies created)
- `data.trophiesAwarded` (total trophies awarded)

**Functionality:**
- Shows Discord stats (servers, users, uptime)
- Shows bot stats (commands, runs, trophies, awards)
- Formatted in two columns

**Response Format:**
- Main color embed with title "Stats"
- Two fields: "Discord" and "Trophies"
- Inline fields for layout

**Implementation Notes:**
- Requires access to client guilds and users cache
- Uptime formatting function needed

---

### 9. `/suggest`
**Description:** Suggest a feature or change for the bot. (Now just an advice to join the support server to suggest)

**SlashCommandBuilder:**
```rust
SlashCommandBuilder::new()
    .name("suggest")
    .description("Suggest a feature or change for the bot. (Now just an advice to join the support server to suggest)")
```

**Parameters:** None

**Permissions:** None required

**Cooldown:** 10 seconds

**Functionality:**
- Redirects users to support server for suggestions
- Migration notice from old suggestion system
- Static response with support server link

**Response Format:**
- Main color embed
- Title: "🫂 Migrating Suggestions"
- Support server link in description

**Implementation Notes:**
- No actual suggestion functionality
- Redirect to external support server

---

### 10. `/support`
**Description:** You need extra help? Join our support server.

**SlashCommandBuilder:**
```rust
SlashCommandBuilder::new()
    .name("support")
    .description("You need extra help? Join our support server.")
```

**Parameters:** None

**Permissions:** None required

**Functionality:**
- Shows support server link
- Links to GitHub issues
- References suggest command
- Ephemeral response

**Response Format:**
- Main color embed with question mark emoji title
- Multiple help options listed
- Bot avatar as thumbnail
- Ephemeral: true

---

## Management Commands (12 commands)

### 1. `/award`
**Description:** Award a trophy for an user.

**SlashCommandBuilder:**
```rust
SlashCommandBuilder::new()
    .name("award")
    .description("Award a trophy for an user.")
    .default_member_permissions(Permissions::MANAGE_GUILD)
    .add_string_option(StringOption::new()
        .name("trophy")
        .description("Name or ID of the trophy to award")
        .required(true))
    .add_user_option(UserOption::new()
        .name("user") 
        .description("User to award the trophy to")
        .required(true))
    .add_integer_option(IntegerOption::new()
        .name("count")
        .description("Number of trophies to award, defaults to 1")
        .required(false))
```

**Parameters:**
- `trophy` (String, required): Trophy name or ID
- `user` (User, required): Target user
- `count` (Integer, optional): Number of trophies (default: 1)

**Permissions:** 
- Discord: Manage Guild
- Custom: `manage_users` (deprecated but checked)

**Validation Rules:**
- Trophy must exist in guild
- Count must be between 1-50 (inclusive)
- Count defaults to 1, minimum enforced as 1

**Database Operations:**
- Read: `data.${guild}.trophies.${id}` (trophy object)
- Read: `data.${guild}.users.${user}.trophies` (user's trophy array)
- Write: `data.${guild}.users.${user}.trophies` (append trophy IDs)
- Add: `data.${guild}.users.${user}.trophyValue` (add trophy value * count)
- Add: `data.trophiesAwarded` (global counter)

**Special Functions:**
- `getTrophy()`: Resolve trophy name/ID to actual ID
- `doRewardRoles()`: Update user's reward roles based on new score

**Response Format:**
- Success: Green embed with trophy emoji and count
- Error: Red embed with error message

**Error Handling:**
- Trophy not found
- Invalid count range (0-50)
- Database operation failures

---

### 2. `/clear`
**Description:** Clear all trophies and resets the score of an user to 0.

**SlashCommandBuilder:**
```rust
SlashCommandBuilder::new()
    .name("clear")
    .description("Clear all trophies and resets the score of an user to 0.")
    .default_member_permissions(Permissions::MANAGE_GUILD)
    .add_user_option(UserOption::new()
        .name("user")
        .description("User to award the trophy to")
        .required(true))
```

**Parameters:**
- `user` (User, required): Target user to clear

**Permissions:**
- Discord: Manage Guild
- Custom: `manage_users` (deprecated)

**Database Operations:**
- Set: `data.${guild}.users.${user}.trophies` = [] (empty array)
- Set: `data.${guild}.users.${user}.trophyValue` = 0

**Special Functions:**
- `doRewardRoles()`: Update user's reward roles (likely remove all)

**Response Format:**
- Success embed mentioning user cleared
- Green color with success emoji

**Implementation Notes:**
- Complete user trophy reset
- Affects role rewards immediately

---

### 3. `/create`
**Description:** Create a new trophy for your server.

**SlashCommandBuilder:**
```rust
SlashCommandBuilder::new()
    .name("create")
    .description("Create a new trophy for your server.")
    .default_member_permissions(Permissions::MANAGE_GUILD)
    .add_string_option(StringOption::new()
        .name("name")
        .description("The name of the trophy.")
        .required(true))
    .add_string_option(StringOption::new()
        .name("description")
        .description("Description for the trophy")
        .required(false))
    .add_string_option(StringOption::new()
        .name("emoji")
        .description("An emoji for the trophy, leave blank for default")
        .required(false))
    .add_number_option(NumberOption::new()
        .name("value")
        .description("How much this trophy values. Defaults to 10")
        .required(false))
    .add_string_option(StringOption::new()
        .name("dedication")
        .description("Dedicate the trophy to someone, defaults to no one. You can use an id or mention as well")
        .required(false))
    .add_boolean_option(BooleanOption::new()
        .name("signed")
        .description("If true, you'll sign this trophy as created by you. Defaults to false")
        .required(false))
    .add_attachment_option(AttachmentOption::new()
        .name("image")
        .description("The image for the trophy, only seen on showcase command")
        .required(false))
    .add_string_option(StringOption::new()
        .name("details")
        .description("Private details for the trophy, you can set why do you give the trophy here.")
        .required(false))
```

**Parameters:**
- `name` (String, required): Trophy name
- `description` (String, optional): Trophy description (default: "No description provided")
- `emoji` (String, optional): Trophy emoji (default: ":trophy:")
- `value` (Number, optional): Trophy point value (default: 10)
- `dedication` (String, optional): Dedication text or user mention
- `signed` (Boolean, optional): Whether to sign trophy (default: false)
- `image` (Attachment, optional): Trophy image file
- `details` (String, optional): Private details (default: "No details provided.")

**Permissions:**
- Discord: Manage Guild  
- Custom: `manage_trophies` (deprecated)

**Validation Rules:**
- Maximum 150 trophies per guild
- Name: Max 32 characters
- Description: Max 128 characters
- Emoji: Max 64 characters
- Value: Between -999999 and 999999
- Dedication: Max 32 characters
- Details: Max 300 characters
- Image: Must be PNG/JPG/JPEG/GIF, max 1MB

**Database Operations:**
- Read: `data.${guild}.trophies` (count existing trophies)
- Add: `data.${guild}.trophies.current` (increment counter)
- Read: `data.${guild}.trophies.current` (get new ID)
- Set: `data.${guild}.trophies.${id}` (full trophy object)
- Add: `data.trophies` (global counter)

**Special Functions:**
- `getTrophyCount()`: Count guild trophies
- `parseUser()`: Parse dedication string to user object
- `downloadImage()`: Save image file to `./images/${guild}_${id}.${extension}`

**Trophy Object Structure:**
```javascript
{
    creator: interaction.user.id,
    created: Date.now(),
    name: name,
    description: desc,
    emoji: emoji,
    value: value,
    image: image?.url ? `${guild}_${next}.${extension}` : null,
    dedication: {
        user: userId || null,
        name: userName || null
    },
    details: details,
    signed: signed
}
```

**Response Format:**
- Success embed with trophy details
- Shows emoji, name, description, value
- Additional fields for signature and dedication
- Image attachment if provided
- Footer with Trophy ID

---

### 4. `/delete`
**Description:** Delete a trophy from your server.

**SlashCommandBuilder:**
```rust
SlashCommandBuilder::new()
    .name("delete")
    .description("Delete a trophy from your server.")
    .default_member_permissions(Permissions::MANAGE_GUILD)
    .add_string_option(StringOption::new()
        .name("trophy")
        .description("Name or ID of the trophy to delete")
        .required(true))
```

**Parameters:**
- `trophy` (String, required): Trophy name or ID

**Permissions:**
- Discord: Manage Guild
- Custom: `manage_trophies` (deprecated)

**Database Operations:**
- Delete: `data.${guild}.trophies.${id}` (trophy object)
- Subtract: `data.trophies` (global counter, min 0)

**Special Functions:**
- `getTrophy()`: Resolve trophy name/ID
- `cleanseTrophies()`: Remove trophy from all users who have it
- File deletion: `./images/${image}` (trophy image)

**Response Format:**
- Success embed with deleted trophy name and emoji
- Green color with success emoji

**Implementation Notes:**
- Removes trophy from all users automatically
- Deletes associated image file
- Updates global trophy counter

---

### 5. `/details`
**Description:** Shows the details of a trophy

**SlashCommandBuilder:**
```rust
SlashCommandBuilder::new()
    .name("details")
    .description("Shows the details of a trophy")
    .default_member_permissions(Permissions::MANAGE_GUILD)
    .add_string_option(StringOption::new()
        .name("trophy")
        .description("Name or ID of the trophy to show")
        .required(true))
```

**Parameters:**
- `trophy` (String, required): Trophy name or ID

**Permissions:** Manage Guild

**Database Operations:**
- Read: `data.${guild}.trophies.${id}` (trophy object)

**Response Format:**
- Embed with trophy emoji and name as title
- Details text in description
- Footer with Trophy ID
- Embed URL: `https://www.youtube.com/watch?v=PwP9ebvCBAM`

**Implementation Notes:**
- Shows private details field of trophy
- Different from `/show` command (public vs private info)

---

### 6. `/edit`
**Description:** Edit an existing trophy for your server.

**SlashCommandBuilder:**
```rust
SlashCommandBuilder::new()
    .name("edit")
    .description("Edit an existing trophy for your server.")
    .default_member_permissions(Permissions::MANAGE_GUILD)
    .add_string_option(StringOption::new()
        .name("trophy")
        .description("The trophy to be edited")
        .required(true))
    .add_string_option(StringOption::new()
        .name("name")
        .description("The new name of the trophy.")
        .required(false))
    .add_string_option(StringOption::new()
        .name("description")
        .description("The new description for the trophy")
        .required(false))
    .add_string_option(StringOption::new()
        .name("emoji")
        .description("A new emoji for the trophy, leave blank for default")
        .required(false))
    .add_string_option(StringOption::new()
        .name("dedication")
        .description("A new dedication for the trophy")
        .required(false))
    .add_string_option(StringOption::new()
        .name("details")
        .description("A new details text for the trophy")
        .required(false))
    .add_attachment_option(AttachmentOption::new()
        .name("image")
        .description("A new image for the trophy")
        .required(false))
```

**Parameters:**
- `trophy` (String, required): Trophy to edit (name or ID)
- `name` (String, optional): New name
- `description` (String, optional): New description
- `emoji` (String, optional): New emoji
- `dedication` (String, optional): New dedication
- `details` (String, optional): New details
- `image` (Attachment, optional): New image

**Permissions:**
- Discord: Manage Guild
- Custom: `manage_trophies` (deprecated)

**Validation Rules:**
- Same as `/create` command for all fields
- Special: dedication can be "-" to remove dedication

**Database Operations:**
- Read: `data.${guild}.trophies.${id}` (existing trophy)
- Set: `data.${guild}.trophies.${id}` (updated trophy)

**Special Functions:**
- `getTrophy()`: Resolve trophy ID
- `parseUser()`: Parse dedication
- `downloadImage()`: Save new image if provided
- Change tracking for response

**Response Format:**
- Shows changes made in structured format
- Lists: Name, Description, Emoji, Value, Dedication, Details, Image changes
- Error if no changes made

**Implementation Notes:**
- Preserves unchanged fields
- Tracks and displays all changes
- Special handling for dedication removal

---

### 7. `/export`
**Description:** Export the bot's data

**SlashCommandBuilder:**
```rust
SlashCommandBuilder::new()
    .name("export")
    .description("Export the bot's data")
    .default_member_permissions(Permissions::ADMINISTRATOR)
```

**Parameters:** None

**Permissions:** Administrator

**Database Operations:**
- Read: `data.${guild}` (entire guild data)

**Functionality:**
- Exports entire guild database as JSON file
- Creates temporary file: `export-${guild}.json`
- Sends as attachment, then deletes local file

**Response Format:**
- File attachment with guild data
- Error message if no data found

**Implementation Notes:**
- Requires file system operations
- Temporary file handling
- JSON serialization of database

---

### 8. `/panel`
**Description:** Create a leaderboard panel. You can only have one panel at a time.

**SlashCommandBuilder:**
```rust
SlashCommandBuilder::new()
    .name("panel")
    .description("Create a leaderboard panel. You can only have one panel at a time.")
    .default_member_permissions(Permissions::MANAGE_GUILD)
    .add_subcommand(SubCommand::new()
        .name("create")
        .description("Create the panel for the leaderboard."))
    .add_subcommand(SubCommand::new()
        .name("delete")
        .description("Delete the panel for the leaderboard."))
```

**Parameters:** Subcommands only

**Permissions:**
- Discord: Manage Guild
- Custom: `manage_users` (deprecated)

**Subcommands:**

#### `/panel create`
**Database Operations:**
- Set: `data.${guild}.panel` = { message: messageId, channel: channelId }

**Special Functions:**
- `updatePanel()`: Create/update leaderboard message
- Creates persistent message in current channel

**Response Format:**
- Deletes interaction reply (ephemeral panel creation)
- Error embed if creation fails

#### `/panel delete`
**Database Operations:**
- Delete: `data.${guild}.panel`

**Response Format:**
- Success embed confirming deletion

**Implementation Notes:**
- Only one panel per guild allowed
- Panel auto-updates periodically
- Requires message editing permissions

---

### 9. `/permissions` (DEPRECATED)
**Description:** Modify the permissions of a role.

**Status:** Shows deprecation warning only
**Implementation:** Should show migration notice to Discord's native permission system

**Response Format:**
- Error embed explaining deprecation
- Instructions to use Discord's permission system
- Link to support server

---

### 10. `/revoke`
**Description:** Revoke a trophy from an user.

**SlashCommandBuilder:**
```rust
SlashCommandBuilder::new()
    .name("revoke")
    .description("Revoke a trophy from an user.")
    .default_member_permissions(Permissions::MANAGE_GUILD)
    .add_string_option(StringOption::new()
        .name("trophy")
        .description("Name or ID of the trophy to revoke")
        .required(true))
    .add_user_option(UserOption::new()
        .name("user")
        .description("User to revoke the trophy from")
        .required(true))
    .add_integer_option(IntegerOption::new()
        .name("count")
        .description("Number of trophies to revoke, defaults to 1.")
        .required(false))
```

**Parameters:**
- `trophy` (String, required): Trophy name or ID
- `user` (User, required): Target user
- `count` (Integer, optional): Number to revoke (default: 1)

**Permissions:**
- Discord: Manage Guild
- Custom: `manage_users` (deprecated)

**Validation Rules:**
- Count must be between 1-50
- Cannot revoke more than user has

**Database Operations:**
- Read: `data.${guild}.users.${user}.trophies` (user's trophies)
- Set: `data.${guild}.users.${user}.trophies` (updated array)
- Subtract: `data.${guild}.users.${user}.trophyValue` (trophy value * count)

**Special Functions:**
- `getTrophy()`: Resolve trophy ID
- `doRewardRoles()`: Update user's reward roles
- Array manipulation: Remove specific number of trophy instances

**Response Format:**
- Success embed with count revoked (or "all" if revoked all instances)
- Shows trophy emoji and name

**Implementation Notes:**
- Removes from end of trophy array using pop()
- Handles partial revocation correctly
- Updates user's total score

---

### 11. `/rewards`
**Description:** Create a new trophy for your server. (Note: Description is incorrect in source)

**SlashCommandBuilder:**
```rust
SlashCommandBuilder::new()
    .name("rewards")
    .description("Create a new trophy for your server.")
    .default_member_permissions(Permissions::MANAGE_GUILD)
    .add_subcommand(SubCommand::new()
        .name("add")
        .description("Add permissions to a role.")
        .add_role_option(RoleOption::new()
            .name("role")
            .description("Which role you want to add as reward.")
            .required(true))
        .add_integer_option(IntegerOption::new()
            .name("requirement")
            .description("How much score the user will require to get this role.")
            .required(true)))
    .add_subcommand(SubCommand::new()
        .name("remove")
        .description("Remove a role reward from your server.")
        .add_role_option(RoleOption::new()
            .name("role")
            .description("Which role you want to remove from rewards")
            .required(true)))
    .add_subcommand(SubCommand::new()
        .name("clear")
        .description("Clears all rewards in this server."))
    .add_subcommand(SubCommand::new()
        .name("list")
        .description("List of reward roles.")
        .add_integer_option(IntegerOption::new()
            .name("page")
            .description("Page to look at")
            .required(false)))
```

**Permissions:**
- Discord: Manage Guild
- Custom: `manage_rewards` (deprecated)

**Subcommands:**

#### `/rewards add`
**Parameters:**
- `role` (Role, required): Role to add as reward
- `requirement` (Integer, required): Score requirement

**Validation Rules:**
- Requirement must be at least 1
- Maximum 20 reward roles per server
- Cannot add duplicate role or requirement
- User cannot add role higher than their highest role (unless owner)

**Database Operations:**
- Read: `data.${guild}.rewards` (existing rewards array)
- Set: `data.${guild}.rewards` (updated, sorted array)

**Reward Object Structure:**
```javascript
{
    role: roleId,
    requirement: requirement
}
```

#### `/rewards remove`
**Parameters:**
- `role` (Role, required): Role to remove

**Validation Rules:**
- Same role hierarchy check as add
- Role must exist in rewards

**Database Operations:**
- Read/Write: `data.${guild}.rewards` (filter out role)

#### `/rewards clear`
**Database Operations:**
- Set: `data.${guild}.rewards` = []

#### `/rewards list`
**Parameters:**
- `page` (Integer, optional): Page number (default: 1)

**Database Operations:**
- Read: `data.${guild}.rewards`
- Read: `data.${guild}.users.${user}.trophyValue` (user's score)

**Response Format:**
- Paginated list (5 per page)
- Shows user's current score
- Format: "**:medal: {requirement}**\n<@&{role}>\n"

**Implementation Notes:**
- Rewards sorted by requirement (descending)
- Role hierarchy validation
- Automatic role management based on user scores

---

### 12. `/settings`
**Description:** Modify the server settings for the bot.

**SlashCommandBuilder:**
```rust
SlashCommandBuilder::new()
    .name("settings")
    .description("Modify the server settings for the bot.")
    .default_member_permissions(Permissions::MANAGE_GUILD)
    .add_subcommand(SubCommand::new()
        .name("set")
        .description("Change a setting for the server.")
        .add_string_option(StringOption::new()
            .name("setting")
            .description("Which setting you want to change.")
            .required(true)
            .add_choices(/* Dynamic from settings array */))
        .add_string_option(StringOption::new()
            .name("value")
            .description("The value you want to set the setting to. Defaults to setting default")
            .required(false)))
    .add_subcommand(SubCommand::new()
        .name("list")
        .description("List all settings of the server"))
```

**Available Settings:**
1. **Dedication Display** (`dedication_display`)
   - Options: ["Always Mention", "Always Name", "Mention Only in Server"]
   - Default: 2 (Mention Only in Server)
   - Description: How to display trophy dedication

2. **Stack Roles** (`stack_roles`)
   - Options: ["Stack Roles", "Only Highest Reward"] 
   - Default: 1 (Only Highest Reward)
   - Description: Role reward behavior

3. **Hide Unused Trophies** (`hide_unused_trophies`)
   - Options: ["Hide Unused Trophies", "Show Unused Trophies"]
   - Default: 1 (Show Unused Trophies)
   - Description: Visibility for users without manage trophies permission

4. **Hide Quit Users** (`hide_quit_users`)
   - Options: ["Hide Quit Users", "Show Quit Users"]
   - Default: 0 (Hide Quit Users)
   - Description: Leaderboard visibility for ex-members

5. **Leaderboard Format** (`leaderboard_format`)
   - Options: ["Mention", "Username", "Nickname", "Username and Tag"]
   - Default: 0 (Mention)
   - Description: User display format on leaderboard

**Subcommands:**

#### `/settings set`
**Parameters:**
- `setting` (String, required): Setting ID (choices from settings array)
- `value` (String, optional): New value (option name or number)

**Database Operations:**
- Set: `data.${guild}.settings.${setting}` (option index)

**Value Parsing:**
- Accepts option numbers (1-based) or option names
- Case-insensitive name matching

#### `/settings list`
**Database Operations:**
- Read: `data.${guild}.settings` (all settings)

**Response Format:**
- Embed with all settings listed
- Shows current value for each setting
- Includes setting descriptions and available options

---

## User-Facing Commands (4 commands)

### 1. `/leaderboard`
**Description:** Shows the server's leaderboard.

**SlashCommandBuilder:**
```rust
SlashCommandBuilder::new()
    .name("leaderboard")
    .description("Shows the server's leaderboard.")
    .add_integer_option(IntegerOption::new()
        .name("page")
        .description("Which page to show. Defaults to 1")
        .required(false))
```

**Parameters:**
- `page` (Integer, optional): Page number (default: 1)

**Permissions:** None required

**Database Operations:**
- Read: `data.${guild}.users` (all users)
- Read: `data.${guild}.settings.hide_quit_users`
- Read: `data.${guild}.settings.leaderboard_format`

**Special Functions:**
- `isInServer()`: Check if user is still in server
- `getSetting()`: Get guild settings
- `parseFormat()`: Format user display based on setting
- `getMedal()`: Get medal emoji for ranking
- `getPage()`: Paginate results (10 per page)
- `attemptFetchIfCacheCleared()`: Refresh user cache

**Functionality:**
- Sorts users by trophyValue (descending)
- Filters based on hide_quit_users setting
- Shows total server score
- Paginates with 10 users per page
- Medal emojis for top positions

**Response Format:**
- Embed with server name in title
- Total server score in description
- Ranked list with medals, positions, names, and scores
- Page information in footer

**Implementation Notes:**
- User cache management critical
- Multiple display format options
- Server member checking required

---

### 2. `/show`
**Description:** Show a trophy.

**SlashCommandBuilder:**
```rust
SlashCommandBuilder::new()
    .name("show")
    .description("Show a trophy.")
    .add_string_option(StringOption::new()
        .name("trophy")
        .description("Name or ID of the trophy to show")
        .required(true))
```

**Parameters:**
- `trophy` (String, required): Trophy name or ID

**Permissions:** None required

**Database Operations:**
- Read: `data.${guild}.trophies.${id}` (trophy object)
- Read: `data.${guild}.settings.dedication_display`

**Special Functions:**
- `getTrophy()`: Resolve trophy name/ID
- `getSetting()`: Get dedication display setting
- `getDedication()`: Format dedication based on setting

**Response Format:**
- Embed with trophy emoji and name as title
- Trophy description in description
- Trophy image (attachment or URL)
- Value field showing medal value
- Optional signed by field
- Optional dedicated to field
- Footer with Trophy ID
- Embed URL: `https://www.youtube.com/watch?v=04854XqcfCY`

**File Handling:**
- Local images: `./images/${image}` as attachment
- Remote images: Direct URL in embed
- Default image if none: `https://cdn.discordapp.com/attachments/631540341148876802/985219082662064178/trophy.png`

**Implementation Notes:**
- Public trophy display (vs `/details` for private)
- Conditional fields based on trophy properties
- Image handling from local files or URLs

---

### 3. `/trophies`
**Description:** See a list of trophies.

**SlashCommandBuilder:**
```rust
SlashCommandBuilder::new()
    .name("trophies")
    .description("See a list of trophies.")
    .add_subcommand(SubCommand::new()
        .name("user")
        .description("Show the trophies any user has.")
        .add_user_option(UserOption::new()
            .name("user")
            .description("User to check trophies")
            .required(false))
        .add_integer_option(IntegerOption::new()
            .name("page")
            .description("Page to look at")
            .required(false)))
    .add_subcommand(SubCommand::new()
        .name("guild")
        .description("Show the trophies any guild has.")
        .add_integer_option(IntegerOption::new()
            .name("page")
            .description("Page to look at")
            .required(false)))
```

**Subcommands:**

#### `/trophies user`
**Parameters:**
- `user` (User, optional): User to check (default: command user)
- `page` (Integer, optional): Page number (default: 1)

**Database Operations:**
- Read: `data.${guild}.users.${user}.trophyValue` (user's score)
- Read: `data.${guild}.users.${user}.trophies` (user's trophy array)
- Read: `data.${guild}.trophies.${id}` (trophy objects)

**Functionality:**
- Aggregates user's trophies by type
- Counts duplicates (e.g., "_x3_")
- Sorts by trophy value (descending)
- Shows total score and trophy count
- Pagination (10 per page)

**Response Format:**
- Title: "{username}'s Trophies"
- Description: Total score
- Trophy list with emoji, name, value, count
- Format: `{emoji} {name} **{value}** _x{count}_`

#### `/trophies guild`
**Parameters:**
- `page` (Integer, optional): Page number (default: 1)

**Database Operations:**
- Read: `data.${guild}.trophies` (all guild trophies)
- Read: `data.${guild}.permissions.manage_trophies` (permission roles)
- Read: `data.${guild}.settings.hide_unused_trophies`
- Read: `data.${guild}.users` (for usage checking)

**Functionality:**
- Lists all guild trophies
- Sorts by value (descending)
- Filters unused trophies for non-managers
- Shows total trophy count
- Pagination (10 per page)

**Permission Checking:**
- Checks if user has manage_trophies permission
- Checks if user is administrator
- Applies filtering based on permissions and settings

**Response Format:**
- Title: "Server Trophies"
- Description: Total trophies created
- Trophy list with emoji, name, value
- Format: `{emoji} {name} **{value}**`

**Implementation Notes:**
- Complex permission and filtering logic
- Trophy usage tracking across all users
- Different views based on user permissions

---

### 4. `/trophystats` (EMPTY/NON-FUNCTIONAL)
**Status:** File exists but is completely empty (0 bytes)
**Implementation:** Should not be implemented in Rust rewrite

---

## Global Settings Reference

The bot uses 5 configurable settings per guild:

1. **Dedication Display** (ID: `dedication_display`)
   - Index 0: Always Mention (`<@userId>`)
   - Index 1: Always Name (username string)
   - Index 2: Mention Only in Server (mention if in server, name if not)
   - Default: 2

2. **Stack Roles** (ID: `stack_roles`)
   - Index 0: Stack Roles (user gets all qualifying roles)
   - Index 1: Only Highest Reward (user gets only highest role they qualify for)
   - Default: 1

3. **Hide Unused Trophies** (ID: `hide_unused_trophies`)
   - Index 0: Hide Unused Trophies (hide from non-managers)
   - Index 1: Show Unused Trophies (show to everyone)
   - Default: 1

4. **Hide Quit Users** (ID: `hide_quit_users`)
   - Index 0: Hide Quit Users (exclude from leaderboard)
   - Index 1: Show Quit Users (include in leaderboard)
   - Default: 0

5. **Leaderboard Format** (ID: `leaderboard_format`)
   - Index 0: Mention (`<@userId>`)
   - Index 1: Username (user.username)
   - Index 2: Nickname (guild nickname)
   - Index 3: Username and Tag (user.username#discriminator)
   - Default: 0

## Permission System

### Discord Native Permissions
Commands use Discord's built-in permission system via `setDefaultMemberPermissions()`:
- "8" = Administrator
- "32" = Manage Guild

### Deprecated Custom Permissions
Legacy system (still checked for safety) with three permission types:
- `manage_users`: Award, revoke, clear trophies
- `manage_trophies`: Create, edit, delete trophies
- `manage_rewards`: Manage role rewards

### Safety Mode
- Commands with custom permissions require `imsafe` flag to be true
- Without `imsafe`, shows migration warning instead of executing
- Set via `/imsafe` command

## Database Schema

### Guild Data Structure
```
data.{guildId} = {
    imsafe: boolean,
    trophies: {
        current: number,
        {id}: {
            creator: string,
            created: number,
            name: string,
            description: string,
            emoji: string,
            value: number,
            image: string|null,
            dedication: {
                user: string|null,
                name: string|null
            },
            details: string,
            signed: boolean
        }
    },
    users: {
        {userId}: {
            trophies: string[], // array of trophy IDs
            trophyValue: number
        }
    },
    settings: {
        dedication_display: number,
        stack_roles: number,
        hide_unused_trophies: number,
        hide_quit_users: number,
        leaderboard_format: number
    },
    rewards: [{
        role: string,
        requirement: number
    }],
    panel: {
        message: string,
        channel: string
    }
}
```

### Bot Data Structure
```
data = {
    version: number,
    defaultLanguage: string,
    bannedUsers: string[],
    commands: {
        total: number,
        {commandName}: number
    },
    trophies: number,
    trophiesAwarded: number
}
```

## Error Handling Patterns

### Common Error Types
1. **Trophy Not Found**: Invalid trophy name/ID
2. **Permission Denied**: User lacks required permissions
3. **Validation Failed**: Input doesn't meet requirements
4. **Database Error**: Database operation failed
5. **File Error**: Image upload/download failed

### Response Patterns
- Success: Green embed with success emoji
- Error: Red embed with error emoji
- Warning: Red/Orange embed with warning emoji

## Implementation Priority

### Essential Commands (Must Have)
1. `/create` - Core trophy creation
2. `/award` - Core trophy awarding
3. `/trophies user` - View user trophies
4. `/leaderboard` - Core leaderboard
5. `/show` - Display trophy details

### Important Commands (Should Have)
1. `/delete` - Trophy management
2. `/edit` - Trophy modification
3. `/revoke` - Trophy removal
4. `/clear` - User reset
5. `/settings` - Bot configuration
6. `/rewards` - Role rewards

### Utility Commands (Nice to Have)
1. `/help` - User guidance
2. `/about` - Bot information
3. `/ping` - Status checking
4. `/stats` - Bot statistics
5. `/support` - Support information

### Administrative Commands (Special Use)
1. `/export` - Data backup
2. `/forgetme` - Data removal
3. `/panel` - Leaderboard panels
4. `/imsafe` - Permission migration

### Deprecated/Unused
1. `/permissions` - Shows deprecation notice
2. `/language` - Completely commented out
3. `/suggest` - Redirects to support server
4. `/trophystats` - Empty file

This documentation provides the complete specification for implementing all Discord slash commands in the Rust rewrite. Each command includes exact parameter definitions, validation rules, database operations, and response formats needed for a functionally equivalent implementation.

Read also @COMMANDS_AND_FUNCTIONALITY.md to understand previous implementation details.
