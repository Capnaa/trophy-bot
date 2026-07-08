//! One file per slash command (implementation-plan C0). `all()` is the single
//! registration point consumed by the framework in `src/bot/mod.rs`.
//!
//! `/ping` and `/about` are fully implemented; the rest are localized
//! "under construction" stubs so the whole command set compiles and
//! registers from day one. Each stub file names the batch that implements it.

mod about;
mod award;
mod clear;
mod create;
mod delete;
mod details;
mod edit;
mod export;
mod forgetme;
mod help;
mod imsafe;
mod invite;
mod leaderboard;
mod panel;
mod permissions;
mod ping;
mod revoke;
mod rewards;
mod settings;
mod show;
mod stats;
mod suggest;
mod support;
mod trophies;

use crate::bot::{Data, Error};

/// Every command the bot registers, in registration order.
pub fn all() -> Vec<poise::Command<Data, Error>> {
    vec![
        // Bot utility
        ping::ping(),
        about::about(),
        help::help(),
        invite::invite(),
        support::support(),
        suggest::suggest(),
        stats::stats(),
        imsafe::imsafe(),
        permissions::permissions(),
        forgetme::forgetme(),
        // Trophy management
        create::create(),
        edit::edit(),
        delete::delete(),
        award::award(),
        revoke::revoke(),
        clear::clear(),
        details::details(),
        // User-facing
        show::show(),
        trophies::trophies(),
        leaderboard::leaderboard(),
        // Server administration
        settings::settings(),
        rewards::rewards(),
        panel::panel(),
        export::export(),
    ]
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use poise::serenity_prelude::Permissions;

    use super::*;

    fn find(commands: &[poise::Command<Data, Error>], name: &str) -> usize {
        commands
            .iter()
            .position(|c| c.name == name)
            .unwrap_or_else(|| panic!("command /{name} not registered"))
    }

    #[test]
    fn registers_all_24_commands() {
        let commands = all();
        assert_eq!(commands.len(), 24);

        let expected = [
            "ping", "about", "help", "invite", "support", "suggest", "stats", "imsafe",
            "permissions", "forgetme", "create", "edit", "delete", "award", "revoke", "clear",
            "details", "show", "trophies", "leaderboard", "settings", "rewards", "panel", "export",
        ];
        for name in expected {
            find(&commands, name);
        }
        assert!(
            !commands.iter().any(|c| c.name == "bench"),
            "old bench command must be gone"
        );
    }

    #[test]
    fn management_commands_require_manage_guild() {
        let commands = all();
        for name in [
            "imsafe", "permissions", "create", "edit", "delete", "award", "revoke", "clear",
            "details", "settings", "rewards", "panel",
        ] {
            let command = &commands[find(&commands, name)];
            assert_eq!(
                command.default_member_permissions,
                Permissions::MANAGE_GUILD,
                "/{name} must default to Manage Guild"
            );
            assert!(command.guild_only, "/{name} must be guild-only");
        }
    }

    #[test]
    fn export_and_forgetme_require_administrator() {
        let commands = all();
        for name in ["export", "forgetme"] {
            let command = &commands[find(&commands, name)];
            assert_eq!(
                command.default_member_permissions,
                Permissions::ADMINISTRATOR,
                "/{name} must default to Administrator"
            );
            assert!(command.guild_only, "/{name} must be guild-only");
        }
    }

    #[test]
    fn user_facing_commands_are_guild_only_without_default_permissions() {
        let commands = all();
        for name in ["show", "trophies", "leaderboard"] {
            let command = &commands[find(&commands, name)];
            assert_eq!(command.default_member_permissions, Permissions::empty());
            assert!(command.guild_only, "/{name} must be guild-only");
        }
    }

    #[test]
    fn stats_and_suggest_have_ten_second_cooldowns() {
        let commands = all();
        for name in ["stats", "suggest"] {
            let command = &commands[find(&commands, name)];
            let config = command.cooldown_config.read().unwrap();
            assert_eq!(
                config.user,
                Some(Duration::from_secs(10)),
                "/{name} must have a 10s user cooldown"
            );
        }
    }

    #[test]
    fn every_command_has_a_description() {
        for command in all() {
            let description = command
                .description
                .as_deref()
                .unwrap_or_else(|| panic!("/{} has no description", command.name));
            assert!(!description.is_empty());
            assert!(
                description.len() <= 100,
                "/{} description exceeds Discord's 100-char limit",
                command.name
            );
        }
    }
}
