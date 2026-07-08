# /leaderboard — en-US catalog. Strings mirror the legacy bot's output
# (commands/users/leaderboard.js) for parity; also used by the panel
# renderer through src/bot/render.rs.

leaderboard-title = 🏆 { $guild }'s Leaderboard
leaderboard-total = Total server score: **{ $total }** :medal:
leaderboard-field-name = Leaderboard
leaderboard-empty = No scores yet
leaderboard-row = { $medal } **{ $rank }.-** { $name } ➤ **{ $score }** :medal:
leaderboard-footer = Page { $page } of { $last }
leaderboard-guild-fallback = Server
