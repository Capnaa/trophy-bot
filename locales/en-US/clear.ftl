# /clear — en-US catalog (batch C7).

clear-cleared = { $count ->
    [0] { $user } had no trophies to clear.
    [one] Successfully cleared **{ $count }** trophy from { $user } and reset their score to 0!
   *[other] Successfully cleared **{ $count }** trophies from { $user } and reset their score to 0!
}
