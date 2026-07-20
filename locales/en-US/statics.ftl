# Static utility commands — en-US catalog (batch: help/invite/support/suggest/
# imsafe/permissions). Keys are prefixed with the command name to stay
# globally unique. Note: Fluent continuation lines must not start with
# "*", "[" or "." — hence the leading bullets/emoji before markdown.

help-title = How to trophies 101
help-description =
    Create custom trophies, award them to members and climb the leaderboard!

    🏆 **Trophy management** (requires Manage Server)
    • `/create` — create a new trophy
    • `/edit` — edit an existing trophy
    • `/delete` — delete a trophy
    • `/award` — award a trophy to a member (inactive trophies can't be given out)
    • `/revoke` — take an awarded trophy back
    • `/clear` — reset a member's trophies and score
    • `/details` — view a trophy's private details

    👥 **For everyone**
    • `/show` — showcase a trophy
    • `/trophies user | guild` — list a member's or the server's trophies
    • `/leaderboard` — ranking of members by score

    ⚙️ **Server setup** (requires Manage Server)
    • `/settings` — configure the bot for this server
    • `/rewards` — automatic role rewards based on score
    • `/panel` — auto-updating leaderboard panel
    • `/panel medals` — auto-updating catalog panel for one category of active trophies
    • `/panel overview` — auto-updating catalog panel for every category at once
    • `/panel retired` — auto-updating catalog panel for every retired (inactive) medal
    • `/link` — mirror another server's panels here, or let another server mirror yours
    • `/export` — export your server's data

    Who can use each command is controlled by Discord itself: open **Server Settings → Integrations → Trophy Bot** to allow or deny commands per role, member or channel.

invite-title = Invite Me to Your Server!
invite-description =
    Want trophies in another server?

    🔗 [Click here to invite Trophy Bot]({ $url })

support-title = ❓ You need support?
support-description =
    Dm .capna on dc

suggest-title = 🫂 Migrating Suggestions
suggest-description =
    The in-bot suggestion system was removed in version 1.4.

    Please share your ideas directly in our [Support Server](https://discord.gg/kNmgU44xgU) — we would love to hear them!

imsafe-safe = ✅ You're currently on safe mode :)

permissions-deprecated-title = ⚠️ Caution!
permissions-deprecated-description =
    The custom permission system is **deprecated** and no longer does anything.

    Use Discord's native slash-command permissions instead: open **Server Settings → Integrations → Trophy Bot** to choose which roles, members and channels may use each command.

    Need a hand? Join the [Support Server](https://discord.gg/kNmgU44xgU).
