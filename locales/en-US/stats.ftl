# /stats catalog (batch C14). All keys are prefixed `stats-`.
# NOTE: Fluent continuation lines must not start with `*`, `[` or `.`.

stats-title = Stats

stats-discord-label = Discord
stats-discord-value =
    Servers (cached): { $servers }
    Users (cached): { $users }
    Uptime: { $uptime }

stats-trophies-label = Trophies
stats-trophies-value =
    Commands: { $commands }
    Runs: { $runs }
    Guilds stored: { $guilds }
    Trophies: { $trophies }
    Awarded: { $awarded }

stats-uptime-value = { $days }d { $hours }h { $minutes }m { $seconds }s
