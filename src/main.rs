mod cli;
mod bot;
mod domain;
mod entities;
mod i18n;
mod import;
mod legacy;
mod migrations;
mod smoke;

use cli::Cli;
use bot::Bot;
use anyhow::Result;

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<()> {
    let args = Cli::parse();

    log::debug!("DB URL: {}", redacted_db_url(&args.database_url));
    if args.command == Some(migrations::MigrateSubcommands::Smoke) {
        // The smoke flow manages its own disposable database; it never uses
        // `--database-url`.
        return smoke::run(&args).await;
    }
    if args.command.is_some() {
        return migrations::cli(args).await;
    }

    Bot::new(&args)
        .await?
        .run()
        .await
}

/// Redacts the userinfo (user:password) portion of a database URL so it can
/// be logged safely: `postgres://user:pass@host/db` becomes
/// `postgres://***@host/db`. URLs without credentials are returned unchanged.
fn redacted_db_url(url: &str) -> String {
    let Some(scheme_end) = url.find("://") else {
        return url.to_string();
    };
    let rest = &url[scheme_end + 3..];
    let authority = &rest[..rest.find('/').unwrap_or(rest.len())];
    match authority.rfind('@') {
        // `at` indexes into `authority`, which is a prefix of `rest`, so
        // `rest[at + 1..]` is everything after the credentials (host + path).
        Some(at) => format!("{}://***@{}", &url[..scheme_end], &rest[at + 1..]),
        None => url.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::redacted_db_url;

    #[test]
    fn redacts_user_and_password() {
        assert_eq!(
            redacted_db_url("postgres://alice:s3cr3t@db.example.com:5432/trophy"),
            "postgres://***@db.example.com:5432/trophy"
        );
    }

    #[test]
    fn redacts_password_containing_at_sign() {
        assert_eq!(
            redacted_db_url("postgres://alice:p@ss@host/db"),
            "postgres://***@host/db"
        );
    }

    #[test]
    fn leaves_credential_free_urls_unchanged() {
        assert_eq!(
            redacted_db_url("sqlite://rdb.sqlite?mode=rwc"),
            "sqlite://rdb.sqlite?mode=rwc"
        );
    }

    #[test]
    fn does_not_treat_at_sign_in_path_as_credentials() {
        assert_eq!(
            redacted_db_url("postgres://host/db@name"),
            "postgres://host/db@name"
        );
    }

    #[test]
    fn leaves_non_url_strings_unchanged() {
        assert_eq!(redacted_db_url("sqlite::memory:"), "sqlite::memory:");
    }

    #[test]
    fn redacts_user_without_password() {
        assert_eq!(
            redacted_db_url("postgres://alice@db.example.com/trophy"),
            "postgres://***@db.example.com/trophy"
        );
    }

    #[test]
    fn redacted_output_never_contains_original_credentials() {
        let url = "postgres://prod_user:hunter2@db.internal:5432/trophy";
        let redacted = redacted_db_url(url);
        assert!(!redacted.contains("prod_user"));
        assert!(!redacted.contains("hunter2"));
    }
}
