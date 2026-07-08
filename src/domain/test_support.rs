//! Shared scaffolding for domain-layer DB tests. Compiled only under
//! `#[cfg(test)]` (see `mod.rs`) so it never ships in the binary.

use chrono::Utc;
use sea_orm::{ActiveModelTrait, ConnectOptions, Database, DatabaseConnection, Set};
use sea_orm_migration::MigratorTrait;

use crate::entities::guilds;
use crate::migrations::Migrator;

/// Fresh in-memory SQLite database with all migrations applied.
///
/// Single connection on purpose: each pooled connection to `sqlite::memory:`
/// would otherwise get its own private database, so a pool larger than one
/// makes queries randomly miss the migrated schema.
pub async fn fresh_db() -> DatabaseConnection {
    let mut options = ConnectOptions::new("sqlite::memory:");
    options.max_connections(1).sqlx_logging(false);
    let db = Database::connect(options)
        .await
        .expect("connect to in-memory sqlite");
    Migrator::fresh(&db).await.expect("apply migrations");
    db
}

/// Current timestamp in the naive-UTC form the entities store.
pub fn now() -> chrono::NaiveDateTime {
    Utc::now().naive_utc()
}

/// Insert a minimal guild row so foreign keys hold.
pub async fn insert_guild(db: &DatabaseConnection, id: i64) {
    guilds::ActiveModel {
        id: Set(id),
        is_safe: Set(true),
        created_at: Set(now()),
        updated_at: Set(now()),
    }
    .insert(db)
    .await
    .expect("insert guild");
}
