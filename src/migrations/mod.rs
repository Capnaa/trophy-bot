use clap::Subcommand;
use sea_orm::{ConnectOptions, Database};
use sea_orm_migration::prelude::*;
use crate::cli::Cli;

mod m20260708_000001_initial_schema;

#[cfg(test)]
mod tests;

pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![
            Box::new(m20260708_000001_initial_schema::Migration)
        ]
    }
}

pub async fn cli(cli: Cli) -> anyhow::Result<()> {
    let url = cli.database_url;

    // Migration progress logs at info level; without this a plain
    // `trophy-bot status` would print nothing (cli sets Warn when not in debug).
    if !cli.debug {
        log::set_max_level(log::LevelFilter::Info);
    }

    let connect_options = ConnectOptions::new(url)
        .sqlx_logging(cli.debug)
        .to_owned();

    let db = Database::connect(connect_options)
        .await
        .expect("Fail to acquire database connection");

    if let Some(MigrateSubcommands::Import { legacy_db }) = &cli.command {
        return crate::import::run(&db, legacy_db).await;
    }

    run_schema_command(&db, cli.command).await
}

/// Runs a schema migration subcommand. Errors are logged and **propagated**
/// so the process exits non-zero on a failed migration — a cutover script
/// chaining `trophy-bot up && trophy-bot import` must stop at the failed `up`
/// instead of importing into a broken schema.
async fn run_schema_command(
    db: &sea_orm::DatabaseConnection,
    command: Option<MigrateSubcommands>,
) -> anyhow::Result<()> {
    let result = match command {
        Some(MigrateSubcommands::Fresh) => Migrator::fresh(db).await,
        Some(MigrateSubcommands::Refresh) => Migrator::refresh(db).await,
        Some(MigrateSubcommands::Reset) => Migrator::reset(db).await,
        Some(MigrateSubcommands::Status) => Migrator::status(db).await,
        Some(MigrateSubcommands::Up { num }) => Migrator::up(db, num).await,
        Some(MigrateSubcommands::Down { num }) => Migrator::down(db, Some(num)).await,
        Some(MigrateSubcommands::Import { .. }) => unreachable!("handled by cli()"),
        Some(MigrateSubcommands::Smoke) => unreachable!("routed to crate::smoke in main"),
        None => Migrator::up(db, None).await,
    };

    if let Err(err) = &result {
        log::error!("{}", err);
    }

    Ok(result?)
}

#[derive(Subcommand, PartialEq, Eq, Debug)]
pub enum MigrateSubcommands {
    #[command(
        about = "Drop all tables from the database, then reapply all migrations",
        display_order = 30
    )]
    Fresh,
    #[command(
        about = "Rollback all applied migrations, then reapply all migrations",
        display_order = 40
    )]
    Refresh,
    #[command(about = "Rollback all applied migrations", display_order = 50)]
    Reset,
    #[command(about = "Check the status of all migrations", display_order = 60)]
    Status,
    #[command(about = "Apply pending migrations", display_order = 70)]
    Up {
        #[arg(short, long, help = "Number of pending migrations to apply")]
        num: Option<u32>,
    },
    #[command(
        about = "Import legacy quick.db data into the normalized schema",
        display_order = 100
    )]
    Import {
        #[arg(
            long,
            default_value = "./json.sqlite",
            help = "Path to the legacy quick.db SQLite file"
        )]
        legacy_db: String,
    },
    #[command(
        about = "Run the end-to-end smoke flow against the test guild on a disposable smoke.sqlite",
        display_order = 110
    )]
    Smoke,
    #[command(about = "Rollback applied migrations", display_order = 80)]
    Down {
        #[arg(
            short,
            long,
            default_value = "1",
            help = "Number of applied migrations to be rolled back",
            display_order = 90
        )]
        num: u32,
    },
}
