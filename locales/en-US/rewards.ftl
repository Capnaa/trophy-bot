# /rewards — en-US catalog (batch C12).
# Wording follows commands-admin.md §/rewards; F25 fixes the wrong
# command/subcommand descriptions (those live in the Rust doc comments).

## /rewards add
rewards-add-success = ✅ Role reward added: reaching a score of **{ $requirement }** now grants { $role }.
rewards-add-error-requirement = The requirement must be at least 1.
rewards-add-error-requirement-too-large = The requirement is too large. The maximum is { $max }.
rewards-add-error-limit = This server already has the maximum of { $max } role rewards.
rewards-add-error-duplicate-role = { $role } is already a role reward. Remove it first if you want to change its requirement.
rewards-add-error-duplicate-requirement = There is already a role reward with a requirement of **{ $requirement }**.

## Hierarchy check (F21), shared by add and remove
rewards-error-hierarchy = You can't manage a reward for { $role } because that role is not below your highest role.

## /rewards remove
rewards-remove-success = ✅ { $role } is no longer a role reward.
rewards-remove-footer = Members who already have this role will keep it; the bot only updates roles when a user's score changes.
rewards-remove-error-not-a-reward = { $role } is not a role reward in this server.
rewards-remove-error-invalid-role = That doesn't look like a role. Pick one from the autocomplete suggestions, or use a role mention or a raw role ID.

## /rewards remove autocomplete choice labels (the sent value is the role ID)
rewards-remove-choice = { $name } (requires { $requirement })
rewards-remove-choice-deleted = { $id } — deleted role (requires { $requirement })

## Shared empty state (remove/clear on a server without rewards)
rewards-error-no-rewards = This server has no role rewards.

## /rewards clear
rewards-clear-success = ✅ Removed { $count ->
        [one] the only role reward
       *[other] all { $count } role rewards
    } from this server.

## /rewards list
rewards-list-title = 🏅 { $guild }'s Role Rewards
rewards-list-fallback-guild-name = Server
rewards-list-description = Your score: **{ $score }**
rewards-list-entry = **🏅 { $requirement }**
    { $role }
rewards-list-deleted-marker = ⚠️ (deleted role)
rewards-list-empty = There are no role rewards in this server yet. Use /rewards add to create one.
rewards-list-footer-page = Page { $page } of { $last }
