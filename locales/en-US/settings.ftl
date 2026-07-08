# /settings — en-US catalog (batch C11).
# Setting names, descriptions and option labels mirror the legacy
# `settings` array in globals.js (option keys are the stored 0-based index).

settings-list-title = ⚙️ { $guild }'s Settings
settings-list-fallback-guild-name = Server
settings-list-footer = Use /settings set <setting> [value] to change a setting.
settings-list-entry-current = **{ $name }:** { $current }
settings-list-entry-description = *{ $description }*
settings-list-entry-options = **Options:** { $options }

settings-set-success = ✅ Setting **{ $name }** changed to **{ $value }**.

settings-dedication-display-name = Dedication Display
settings-dedication-display-description = How to display the dedication of a trophy when it is given.
settings-dedication-display-option-0 = Always Mention
settings-dedication-display-option-1 = Always Name
settings-dedication-display-option-2 = Mention Only in Server

settings-stack-roles-name = Stack Roles
settings-stack-roles-description = When true, the role rewards will stack instead of only adding the highest role reward at a time.
settings-stack-roles-option-0 = Stack Roles
settings-stack-roles-option-1 = Only Highest Reward

settings-hide-unused-trophies-name = Hide Unused Trophies
settings-hide-unused-trophies-description = If true, any trophies that were not given to anyone will be hidden for users without the Manage Server permission.
settings-hide-unused-trophies-option-0 = Hide Unused Trophies
settings-hide-unused-trophies-option-1 = Show Unused Trophies

settings-hide-quit-users-name = Hide Quit Users
settings-hide-quit-users-description = If true, any users that quit the server will be hidden from the leaderboard.
settings-hide-quit-users-option-0 = Hide Quit Users
settings-hide-quit-users-option-1 = Show Quit Users

settings-leaderboard-format-name = Leaderboard Format
settings-leaderboard-format-description = How to display users on the leaderboard. (If there are issues on phones, try changing this to any other than mention.)
settings-leaderboard-format-option-0 = Mention
settings-leaderboard-format-option-1 = Username
settings-leaderboard-format-option-2 = Nickname
settings-leaderboard-format-option-3 = Username and Tag
