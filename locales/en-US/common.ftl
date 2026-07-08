# Trophy Bot — en-US shared framework strings (command framework C0).
# Keys are prefixed `common-` to never collide with per-command catalogs.

common-under-construction-title = 🚧 Under construction
common-under-construction = This command is being rebuilt and is not available yet. Check back soon!

common-error-title = ❌ Something went wrong
common-error-generic = Something went wrong while running this command. Please try again later.
common-error-cooldown = { $seconds ->
    [one] Slow down! You can use this command again in { $seconds } second.
   *[other] Slow down! You can use this command again in { $seconds } seconds.
}
common-error-guild-only = This command can only be used inside a server.
common-error-not-owner = Only the bot owner can use this command.
common-error-missing-user-permissions = You don't have the permissions required to use this command.
common-error-missing-bot-permissions = I'm missing the permissions I need to do that: { $permissions }
common-error-invalid-input = Invalid input. Please check the command arguments and try again.
