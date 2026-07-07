//! Legacy data importer (`trophy-bot import`): quick.db JSON blobs → normalized
//! schema. Algorithm and expected counts: `docs/specs/migration-import.md`.

use sea_orm::DatabaseConnection;

pub async fn run(_db: &DatabaseConnection, legacy_db_path: &str) -> anyhow::Result<()> {
    anyhow::bail!("import not implemented yet (legacy source: {legacy_db_path})")
}
