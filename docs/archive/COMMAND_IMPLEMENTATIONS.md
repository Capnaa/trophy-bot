# Command Implementation Map

Quick reference that links every documented slash command to the JavaScript file that implements it in the legacy Node.js bot. Subcommands live in the same file as their parent command unless noted.

## Bot Utility Commands
- `/about` → `commands/bot/about.js`
- `/forgetme` → `commands/bot/forgetme.js`
- `/help` → `commands/bot/help.js`
- `/imsafe` → `commands/bot/imsafe.js`
- `/invite` → `commands/bot/invite.js`
- `/language` (deprecated/disabled) → `commands/bot/language.js`
- `/ping` → `commands/bot/ping.js`
- `/stats` → `commands/bot/stats.js`
- `/suggest` → `commands/bot/suggest.js`
- `/support` → `commands/bot/support.js`

## Management Commands
- `/award` → `commands/manage/award.js`
- `/clear` → `commands/manage/clear.js`
- `/create` → `commands/manage/create.js`
- `/delete` → `commands/manage/delete.js`
- `/details` → `commands/manage/details.js`
- `/edit` → `commands/manage/edit.js`
- `/export` → `commands/manage/export.js`
- `/panel` (subcommands `create`/`delete`) → `commands/manage/panel.js`
- `/permissions` (deprecated) → `commands/manage/perms.js`
- `/revoke` → `commands/manage/revoke.js`
- `/rewards` (subcommands `add`/`remove`/`clear`/`list`) → `commands/manage/rewards.js`
- `/settings` (subcommands `set`/`list`) → `commands/manage/settings.js`

## User-Facing Commands
- `/leaderboard` → `commands/users/leaderboard.js`
- `/show` → `commands/users/show.js`
- `/trophies` (subcommands `user`/`guild`) → `commands/users/trophies.js`
- `/trophystats` (placeholder, currently empty) → `commands/users/trophystats.js`

> **Note:** All shared helpers (e.g., `getTrophy`, `doRewardRoles`, `cleanseTrophies`) live in `globals.js` and are imported by the command files above. Any future changes should keep this map updated so contributors can quickly locate the legacy implementation of each slash command.
