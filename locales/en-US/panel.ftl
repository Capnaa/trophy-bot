# /panel — leaderboard panel management (batch C13).
# Keys are prefixed with the command name; panel CONTENT itself reuses the
# shared leaderboard-* keys (locales/en-US/leaderboard.ftl).

panel-created = ✅ Panel created in this channel. It refreshes automatically when scores change.
panel-create-failed = I couldn't send the panel message in this channel. Make sure I can send messages and embeds here, then try again.
panel-deleted = ✅ Successfully **deleted** the panel.
panel-delete-none = There is no leaderboard panel in this server.

# /panel medals create|delete — medals CONTENT reuses the shared
# medals-panel-* keys (locales/en-US/medals_panel.ftl).
panel-medals-created = ✅ Panel created in this channel for **{ $category }**. It refreshes automatically when medals in that category change.
panel-medals-create-failed = I couldn't send the panel message in this channel. Make sure I can send messages and embeds here, then try again.
panel-medals-deleted = ✅ Successfully **deleted** the **{ $category }** panel.
panel-medals-delete-none = There is no panel for category "{ $category }" in this server.
