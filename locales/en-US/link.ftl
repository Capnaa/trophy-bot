# /link — cross-guild panel mirroring (guild_links).

link-error-invalid-guild-id = That doesn't look like a valid server ID.
link-error-guild-not-found = I'm not in that server, or the ID is wrong.
link-request-sent = ✅ Request sent. An admin in that server needs to run `/link accept` before your panels can mirror its medals.
link-error-self-link = A server can't link to itself.
link-error-already-linked = This server already has a pending or accepted link. Revoke it first with `/link revoke` before requesting a new one.
link-accepted = ✅ Link accepted. That server's panels will now mirror this server's medals — trophy names, emoji, descriptions and values become visible there (private trophy details never are).
link-error-no-such-request = There's no pending request from that server.
link-error-nothing-to-revoke = There's no link to revoke.
link-revoked = ✅ Link revoked. Any panels mirroring the other server's data have been removed.
link-status-none = This server has no active or pending link.
link-status-linked-to = This server's panels mirror **{ $guild }**.
link-status-pending-to = Waiting on **{ $guild }** to accept this server's link request.
link-status-linked-from = **{ $guild }** mirrors this server's panels.
link-status-pending-from = **{ $guild }** has requested to mirror this server's panels — run `/link accept` to allow it.
