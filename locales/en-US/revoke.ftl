# /revoke — en-US catalog (batch C6).

revoke-error-count = The count must be between { $min } and { $max }.
revoke-error-not-found = Could not find a trophy named "{ $input }" in this server. Try picking one from the autocomplete suggestions.
# F2: explicit message instead of the legacy fake "removed all" success.
revoke-error-none = { $user } does not have the trophy { $emoji } **{ $name }**, so there is nothing to revoke.
# F2: reports the REAL number of copies removed (may be fewer than requested).
revoke-revoked = { $count ->
    [one] Successfully revoked **{ $count }** trophy of { $emoji } **{ $name }** from { $user }!
   *[other] Successfully revoked **{ $count }** trophies of { $emoji } **{ $name }** from { $user }!
}
