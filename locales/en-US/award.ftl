# /award — en-US catalog (batch C3).

award-error-count = The count must be between { $min } and { $max }.
award-error-not-found = Could not find a trophy named "{ $input }" in this server. Try picking one from the autocomplete suggestions.
# Named `award-awarded` because main.ftl holds a scaffold `award-success`
# example key and Fluent message ids must be unique across the bundle.
award-awarded = { $count ->
    [one] Successfully awarded **{ $count }** trophy of { $emoji } **{ $name }** to { $user }!
   *[other] Successfully awarded **{ $count }** trophies of { $emoji } **{ $name }** to { $user }!
}
