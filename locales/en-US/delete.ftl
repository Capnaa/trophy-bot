# /delete — en-US catalog (batch C9; confirmation flow per the spec's Rust
# target, buttons handled in src/bot/buttons.rs).

delete-error-not-found = Could not find a trophy named "{ $input }" in this server. Try picking one from the autocomplete suggestions.
delete-success = Successfully **deleted** trophy { $emoji } **{ $name }**.

## Confirmation step (destructive delete)
delete-confirm-title = ⚠️ Delete { $emoji } { $name }?
delete-confirm-description = Deleting this trophy also removes { $awards ->
        [0] any awards of it (nobody holds it right now)
        [one] its **1 existing award** from a member's collection
       *[other] its **{ $awards } existing awards** from members' collections
    }. This **cannot be undone**. The buttons expire in { $seconds } seconds.
delete-button-confirm = Delete it
delete-button-cancel = Cancel

## Button press outcomes
delete-not-invoker = Only the member who ran /delete can use these buttons.
delete-cancelled-title = Deletion cancelled
delete-cancelled = The trophy was not deleted.
delete-expired-title = Confirmation expired
delete-expired = This confirmation expired without a decision, so nothing was deleted. Run /delete again if you still want to remove the trophy.
delete-gone-title = Trophy not found
delete-gone = That trophy no longer exists — it may have already been deleted.
