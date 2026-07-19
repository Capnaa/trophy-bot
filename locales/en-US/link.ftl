# /link — cross-guild co-administration (guild_links).

link-error-invalid-guild-id = That doesn't look like a valid server ID.
link-error-guild-not-found = I'm not in that server, or the ID is wrong.
link-request-sent = ✅ Request sent. An admin in that server needs to run `/link accept` before this server can manage its medals.
link-error-self-link = A server can't link to itself.
link-error-already-linked = This server already has a pending or accepted link. Revoke it first with `/link revoke` before requesting a new one.
link-accepted = ✅ Link accepted. That server can now fully manage this server's medals — create, edit, delete, award, revoke and clear trophies, and its panels will mirror this server's data — exactly as if it were an admin here. Revoke anytime with `/link revoke`.
link-error-no-such-request = There's no pending request from that server.
link-error-nothing-to-revoke = There's no link to revoke.
link-revoked = ✅ Link revoked. The other server can no longer manage this server's medals, and any panels mirroring its data have been removed.
link-status-none = This server has no active or pending link.
link-status-linked-to = This server fully manages **{ $guild }**'s medals (trophies, awards, panels) as if it were an admin there.
link-status-pending-to = Waiting on **{ $guild }** to accept this server's link request.
link-status-linked-from = **{ $guild }** can fully manage this server's medals, as if it were an admin here.
link-status-pending-from = **{ $guild }** has requested full management of this server's medals — run `/link accept` to allow it.
