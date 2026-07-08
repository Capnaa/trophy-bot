# /export — en-US catalog (batch C15).
# Keys are prefixed `export-` to never collide with other catalogs.

export-title = 📦 Server data export
export-description = Attached is a JSON export of this server's Trophy Bot data: { $trophies ->
    [one] { $trophies } trophy
   *[other] { $trophies } trophies
}, { $awards ->
    [one] { $awards } award
   *[other] { $awards } awards
} and { $rewards ->
    [one] { $rewards } role reward
   *[other] { $rewards } role rewards
}.
