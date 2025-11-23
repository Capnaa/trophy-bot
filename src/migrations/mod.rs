use clap::Subcommand;
use sea_orm::{ConnectOptions, Database};
use sea_orm_migration::prelude::*;
use crate::cli::Cli;

mod m20251115_000001_create_basic_tables;

pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![
            Box::new(m20251115_000001_create_basic_tables::Migration)
        ]
    }
}

pub async fn cli(cli: Cli) -> anyhow::Result<()> {
    let url = cli.database_url;

    let connect_options = ConnectOptions::new(url)
        .to_owned();

    let db = Database::connect(connect_options)
        .await
        .expect("Fail to acquire database connection");

    if let Err(err) = cli::run_migrate(Migrator, &db, cli.command.map(sea_orm_cli::MigrateSubcommands::from), cli.debug).await {
        log::error!("{}", err);
    }

    Ok(())
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

impl From<MigrateSubcommands> for sea_orm_cli::MigrateSubcommands {
    fn from(cmd: MigrateSubcommands) -> Self {
        match cmd {
            MigrateSubcommands::Fresh => sea_orm_cli::MigrateSubcommands::Fresh,
            MigrateSubcommands::Refresh => sea_orm_cli::MigrateSubcommands::Refresh,
            MigrateSubcommands::Reset => sea_orm_cli::MigrateSubcommands::Reset,
            MigrateSubcommands::Status => sea_orm_cli::MigrateSubcommands::Status,
            MigrateSubcommands::Up { num } => sea_orm_cli::MigrateSubcommands::Up { num },
            MigrateSubcommands::Down { num } => sea_orm_cli::MigrateSubcommands::Down { num },
        }
    }
}
