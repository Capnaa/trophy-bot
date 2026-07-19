//! Cross-guild link consent (schema.md `guild_links`): a linked guild ("B")
//! can mirror at most one source guild's ("A") data at a time, and only
//! after A explicitly accepts B's request. Once accepted, B becomes a full
//! second control room for A — every trophy-content command run in B
//! ([`effective_guild`], via `util::effective_guild_id`) reads and writes
//! A's trophies directly (create/edit/delete/award/revoke/clear/details/
//! show/trophies/leaderboard), not just its panels. `accepted_at` doubles
//! as the status flag (NULL = pending, set = accepted).

use sea_orm::sea_query::OnConflict;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DatabaseConnection, EntityTrait,
    PaginatorTrait, QueryFilter, QueryOrder, Set, TransactionTrait,
};
use uuid::Uuid;

use crate::entities::{guild_links, guilds};

// ---------------------------------------------------------------------------
// Reads
// ---------------------------------------------------------------------------

/// The guild `linked_guild_id` currently mirrors, if any (accepted only — a
/// pending request grants nothing).
pub async fn accepted_source_for(
    db: &impl ConnectionTrait,
    linked_guild_id: i64,
) -> Result<Option<i64>, sea_orm::DbErr> {
    let row = guild_links::Entity::find()
        .filter(guild_links::Column::LinkedGuildId.eq(linked_guild_id))
        .filter(guild_links::Column::AcceptedAt.is_not_null())
        .one(db)
        .await?;
    Ok(row.map(|r| r.source_guild_id))
}

/// The guild a command run in `own_guild_id` should actually operate
/// on: the linked source if one is accepted, else `own_guild_id` itself.
/// The single primitive every trophy-content command routes through
/// (`util::effective_guild_id`) — unlinked guilds get back exactly what
/// they passed in, so this is a no-op for the overwhelming majority of
/// guilds that never link.
pub async fn effective_guild(
    db: &impl ConnectionTrait,
    own_guild_id: i64,
) -> Result<i64, sea_orm::DbErr> {
    Ok(accepted_source_for(db, own_guild_id).await?.unwrap_or(own_guild_id))
}

/// Re-validation used at every panel render: true only for an exact,
/// currently-accepted `(source_guild_id, linked_guild_id)` pair. Not just
/// trusting a panel's stored `source_guild_id` — a revoked link must stop
/// leaking data on the very next refresh even if revoke-time cleanup ever
/// missed a row.
pub async fn is_accepted(
    db: &impl ConnectionTrait,
    source_guild_id: i64,
    linked_guild_id: i64,
) -> Result<bool, sea_orm::DbErr> {
    let count = guild_links::Entity::find()
        .filter(guild_links::Column::SourceGuildId.eq(source_guild_id))
        .filter(guild_links::Column::LinkedGuildId.eq(linked_guild_id))
        .filter(guild_links::Column::AcceptedAt.is_not_null())
        .count(db)
        .await?;
    Ok(count > 0)
}

/// This guild's own link row, in EITHER role (as the linked side, at most
/// one; as the source side, this only ever returns one of possibly several —
/// callers wanting all of a source's links use [`linked_guilds`]). Used by
/// `/link status` and by `/link revoke` when run from the linked side, where
/// no `guild_id` parameter is needed.
pub async fn link_as_linked_guild(
    db: &impl ConnectionTrait,
    linked_guild_id: i64,
) -> Result<Option<guild_links::Model>, sea_orm::DbErr> {
    guild_links::Entity::find()
        .filter(guild_links::Column::LinkedGuildId.eq(linked_guild_id))
        .one(db)
        .await
}

/// Pending requesters for a source guild (guilds asking to mirror it),
/// most recent first — feeds `/link accept`'s autocomplete.
pub async fn pending_requesters(
    db: &impl ConnectionTrait,
    source_guild_id: i64,
) -> Result<Vec<i64>, sea_orm::DbErr> {
    let rows = guild_links::Entity::find()
        .filter(guild_links::Column::SourceGuildId.eq(source_guild_id))
        .filter(guild_links::Column::AcceptedAt.is_null())
        .order_by_desc(guild_links::Column::CreatedAt)
        .all(db)
        .await?;
    Ok(rows.into_iter().map(|r| r.linked_guild_id).collect())
}

/// Guilds currently mirroring (accepted) a source guild's panels, most
/// recent first — feeds `/link revoke`'s autocomplete on the source side.
pub async fn linked_guilds(
    db: &impl ConnectionTrait,
    source_guild_id: i64,
) -> Result<Vec<i64>, sea_orm::DbErr> {
    let rows = guild_links::Entity::find()
        .filter(guild_links::Column::SourceGuildId.eq(source_guild_id))
        .filter(guild_links::Column::AcceptedAt.is_not_null())
        .order_by_desc(guild_links::Column::CreatedAt)
        .all(db)
        .await?;
    Ok(rows.into_iter().map(|r| r.linked_guild_id).collect())
}

// ---------------------------------------------------------------------------
// Writes
// ---------------------------------------------------------------------------

/// Everything that can make `/link request` refuse.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestError {
    /// A guild cannot link to itself.
    SelfLink,
    /// This guild is already the linked side of a pending or accepted row
    /// (`UNIQUE(linked_guild_id)` — one source at a time).
    AlreadyLinked,
}

/// Creates a pending link request. Auto-registers both guild rows (FK)
/// without clobbering existing ones, same upsert pattern as
/// `panel_updater::save_panel`. The inner `Err` is the race-window
/// duplicate the unique index catches.
pub async fn request_link(
    db: &DatabaseConnection,
    source_guild_id: i64,
    linked_guild_id: i64,
    requested_by: i64,
) -> anyhow::Result<Result<(), RequestError>> {
    if source_guild_id == linked_guild_id {
        return Ok(Err(RequestError::SelfLink));
    }

    let now = chrono::Utc::now().naive_utc();
    let txn = db.begin().await?;

    for guild_id in [source_guild_id, linked_guild_id] {
        guilds::Entity::insert(guilds::ActiveModel {
            id: Set(guild_id),
            is_safe: Set(false),
            created_at: Set(now),
            updated_at: Set(now),
        })
        .on_conflict(OnConflict::column(guilds::Column::Id).do_nothing().to_owned())
        .exec_without_returning(&txn)
        .await?;
    }

    let inserted = guild_links::ActiveModel {
        id: Set(Uuid::now_v7()),
        source_guild_id: Set(source_guild_id),
        linked_guild_id: Set(linked_guild_id),
        requested_by: Set(requested_by),
        accepted_by: Set(None),
        accepted_at: Set(None),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(&txn)
    .await;

    match inserted {
        Ok(_) => {
            txn.commit().await?;
            Ok(Ok(()))
        }
        Err(err)
            if matches!(err.sql_err(), Some(sea_orm::SqlErr::UniqueConstraintViolation(_))) =>
        {
            txn.rollback().await.ok();
            Ok(Err(RequestError::AlreadyLinked))
        }
        Err(err) => {
            txn.rollback().await.ok();
            Err(err.into())
        }
    }
}

/// Everything that can make `/link accept` refuse.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcceptError {
    /// No pending request from `linked_guild_id` to `source_guild_id`.
    NoSuchRequest,
}

/// Accepts a pending request. Only touches a row that is still pending
/// (`accepted_at IS NULL`) — accepting an already-accepted link is a no-op
/// reported as [`AcceptError::NoSuchRequest`], not silently re-accepted.
pub async fn accept_link(
    db: &DatabaseConnection,
    source_guild_id: i64,
    linked_guild_id: i64,
    accepted_by: i64,
) -> anyhow::Result<Result<(), AcceptError>> {
    let now = chrono::Utc::now().naive_utc();
    let result = guild_links::Entity::update_many()
        .set(guild_links::ActiveModel {
            accepted_at: Set(Some(now)),
            accepted_by: Set(Some(accepted_by)),
            updated_at: Set(now),
            ..Default::default()
        })
        .filter(guild_links::Column::SourceGuildId.eq(source_guild_id))
        .filter(guild_links::Column::LinkedGuildId.eq(linked_guild_id))
        .filter(guild_links::Column::AcceptedAt.is_null())
        .exec(db)
        .await?;
    Ok(if result.rows_affected > 0 { Ok(()) } else { Err(AcceptError::NoSuchRequest) })
}

/// Deletes the link between the two guilds (pending or accepted). Returns
/// whether a row existed. Callers are responsible for cleaning up any
/// panels the linked guild had pointed at the source (see
/// `panel_updater`/`medals_panel`'s `remove_linked_panel(s)`).
pub async fn revoke_link(
    db: &DatabaseConnection,
    source_guild_id: i64,
    linked_guild_id: i64,
) -> anyhow::Result<bool> {
    let result = guild_links::Entity::delete_many()
        .filter(guild_links::Column::SourceGuildId.eq(source_guild_id))
        .filter(guild_links::Column::LinkedGuildId.eq(linked_guild_id))
        .exec(db)
        .await?;
    Ok(result.rows_affected > 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::test_support::fresh_db;

    // --- request_link ---

    #[tokio::test]
    async fn request_link_creates_a_pending_row_and_registers_both_guilds() {
        let db = fresh_db().await;
        let result = request_link(&db, 1, 2, 42).await.expect("infra ok");
        assert_eq!(result, Ok(()));

        assert_eq!(accepted_source_for(&db, 2).await.unwrap(), None, "pending grants nothing");
        assert_eq!(pending_requesters(&db, 1).await.unwrap(), vec![2]);

        let guild1 = guilds::Entity::find_by_id(1).one(&db).await.unwrap();
        let guild2 = guilds::Entity::find_by_id(2).one(&db).await.unwrap();
        assert!(guild1.is_some() && guild2.is_some(), "both guild rows auto-registered");
    }

    // --- effective_guild ---

    #[tokio::test]
    async fn effective_guild_is_the_source_only_when_accepted() {
        let db = fresh_db().await;
        assert_eq!(effective_guild(&db, 2).await.unwrap(), 2, "unlinked guild is its own effective guild");

        request_link(&db, 1, 2, 42).await.unwrap().unwrap();
        assert_eq!(effective_guild(&db, 2).await.unwrap(), 2, "still pending: no redirect yet");

        accept_link(&db, 1, 2, 99).await.unwrap().unwrap();
        assert_eq!(effective_guild(&db, 2).await.unwrap(), 1, "accepted: redirects to the source");

        revoke_link(&db, 1, 2).await.unwrap();
        assert_eq!(effective_guild(&db, 2).await.unwrap(), 2, "revoked: back to its own guild");
    }

    #[tokio::test]
    async fn request_link_rejects_linking_a_guild_to_itself() {
        let db = fresh_db().await;
        let result = request_link(&db, 1, 1, 42).await.expect("infra ok");
        assert_eq!(result, Err(RequestError::SelfLink));
    }

    #[tokio::test]
    async fn request_link_rejects_a_second_link_for_the_same_linked_guild() {
        let db = fresh_db().await;
        request_link(&db, 1, 2, 42).await.unwrap().unwrap();

        // Same linked guild (2) requesting a DIFFERENT source (3): still
        // rejected — unique on linked_guild_id, one source at a time.
        let result = request_link(&db, 3, 2, 42).await.expect("infra ok");
        assert_eq!(result, Err(RequestError::AlreadyLinked));

        // A different linked guild (4) requesting the same source (1) is fine
        // — one-to-many from the source.
        let result = request_link(&db, 1, 4, 42).await.expect("infra ok");
        assert_eq!(result, Ok(()));
    }

    // --- accept_link ---

    #[tokio::test]
    async fn accept_link_flips_a_pending_request_to_accepted() {
        let db = fresh_db().await;
        request_link(&db, 1, 2, 42).await.unwrap().unwrap();

        let result = accept_link(&db, 1, 2, 99).await.expect("infra ok");
        assert_eq!(result, Ok(()));
        assert_eq!(accepted_source_for(&db, 2).await.unwrap(), Some(1));
        assert_eq!(linked_guilds(&db, 1).await.unwrap(), vec![2]);
        assert!(pending_requesters(&db, 1).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn accept_link_reports_when_there_is_nothing_pending() {
        let db = fresh_db().await;
        // No request at all.
        assert_eq!(
            accept_link(&db, 1, 2, 99).await.expect("infra ok"),
            Err(AcceptError::NoSuchRequest)
        );

        // Already accepted: re-accepting is not a no-op success.
        request_link(&db, 1, 2, 42).await.unwrap().unwrap();
        accept_link(&db, 1, 2, 99).await.unwrap().unwrap();
        assert_eq!(
            accept_link(&db, 1, 2, 99).await.expect("infra ok"),
            Err(AcceptError::NoSuchRequest)
        );
    }

    // --- revoke_link ---

    #[tokio::test]
    async fn revoke_link_removes_the_row_and_reopens_the_linked_guild() {
        let db = fresh_db().await;
        request_link(&db, 1, 2, 42).await.unwrap().unwrap();
        accept_link(&db, 1, 2, 99).await.unwrap().unwrap();

        assert!(revoke_link(&db, 1, 2).await.expect("revoke"));
        assert_eq!(accepted_source_for(&db, 2).await.unwrap(), None);
        assert!(!revoke_link(&db, 1, 2).await.expect("revoke again"), "already gone");

        // The linked guild can now link elsewhere.
        let result = request_link(&db, 3, 2, 42).await.expect("infra ok");
        assert_eq!(result, Ok(()));
    }

    // --- is_accepted (render-time re-validation) ---

    #[tokio::test]
    async fn is_accepted_is_false_for_pending_wrong_pair_or_revoked() {
        let db = fresh_db().await;
        request_link(&db, 1, 2, 42).await.unwrap().unwrap();

        assert!(!is_accepted(&db, 1, 2).await.unwrap(), "still pending");
        accept_link(&db, 1, 2, 99).await.unwrap().unwrap();
        assert!(is_accepted(&db, 1, 2).await.unwrap());
        assert!(!is_accepted(&db, 9, 2).await.unwrap(), "wrong source");
        assert!(!is_accepted(&db, 1, 9).await.unwrap(), "wrong linked guild");

        revoke_link(&db, 1, 2).await.unwrap();
        assert!(!is_accepted(&db, 1, 2).await.unwrap(), "revoked");
    }

    // --- link_as_linked_guild ---

    #[tokio::test]
    async fn link_as_linked_guild_finds_the_row_regardless_of_status() {
        let db = fresh_db().await;
        assert!(link_as_linked_guild(&db, 2).await.unwrap().is_none());

        request_link(&db, 1, 2, 42).await.unwrap().unwrap();
        let row = link_as_linked_guild(&db, 2).await.unwrap().expect("pending row found");
        assert_eq!(row.source_guild_id, 1);
        assert!(row.accepted_at.is_none());

        accept_link(&db, 1, 2, 99).await.unwrap().unwrap();
        let row = link_as_linked_guild(&db, 2).await.unwrap().expect("accepted row found");
        assert!(row.accepted_at.is_some());
    }
}
