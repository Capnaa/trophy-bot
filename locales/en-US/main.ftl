# Trophy Bot — en-US message catalog (Fluent, ADR 0010).
# Key naming: <command-or-area>-<message>. Every user-facing string lives here;
# command code never builds user-facing text with format!.

# Generic
error-generic = Something went wrong. Please try again later.
error-trophy-not-found = No trophy named « { $name } » exists in this server.

# /bench (demo command)
bench-response = Latency: { $latency } ms · handler time: { $time } s

# Example with plural selection (pattern for /award and friends)
award-success = { $count ->
    [one] Awarded { $trophy } to { $user }.
   *[other] Awarded { $trophy } ×{ $count } to { $user }.
}
