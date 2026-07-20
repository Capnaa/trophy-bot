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

# /panel overview create|delete — overview CONTENT reuses the shared
# medals-overview-* keys (locales/en-US/medals_panel.ftl).
panel-overview-created = ✅ Panel created in this channel. It refreshes automatically when any category's medals change.
panel-overview-create-failed = I couldn't send the panel message in this channel. Make sure I can send messages and embeds here, then try again.
panel-overview-deleted = ✅ Successfully **deleted** the overview panel.
panel-overview-delete-none = There is no overview panel in this server.

# /panel retired create|delete — retired-overview CONTENT reuses the shared
# medals-retired-* keys (locales/en-US/medals_panel.ftl).
panel-retired-created = ✅ Panel created in this channel. It refreshes automatically when any category's medals change.
panel-retired-create-failed = I couldn't send the panel message in this channel. Make sure I can send messages and embeds here, then try again.
panel-retired-deleted = ✅ Successfully **deleted** the retired medals panel.
panel-retired-delete-none = There is no retired medals panel in this server.
