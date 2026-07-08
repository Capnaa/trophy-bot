//! `/permissions add|list|remove` — DEPRECATED
//! (spec: docs/specs/commands-admin.md §/permissions + rust-parity-plan.md).
//!
//! The custom permission system is not reimplemented. All three subcommands
//! reply with the same static deprecation notice pointing at Discord's
//! native Integrations permissions. No options are exposed (the legacy
//! choice/role options were a QUIRK — inputs were always ignored) and no
//! data operations happen; `data.${guild}.permissions` is not migrated.

use poise::serenity_prelude as serenity;

use crate::bot::{Context, Error, util};
use crate::i18n;

/// Modify the permissions of a role. (Deprecated)
#[poise::command(
    slash_command,
    guild_only,
    default_member_permissions = "MANAGE_GUILD",
    required_permissions = "MANAGE_GUILD",
    subcommands("add", "list", "remove")
)]
pub async fn permissions(_ctx: Context<'_>) -> Result<(), Error> {
    // Never invoked: parent commands with subcommands are not callable.
    Ok(())
}

/// Add permissions to a role. (Deprecated)
#[poise::command(slash_command)]
async fn add(ctx: Context<'_>) -> Result<(), Error> {
    deprecation_notice(ctx).await
}

/// List all permissions. (Deprecated)
#[poise::command(slash_command)]
async fn list(ctx: Context<'_>) -> Result<(), Error> {
    deprecation_notice(ctx).await
}

/// Remove permissions from a role. (Deprecated)
#[poise::command(slash_command)]
async fn remove(ctx: Context<'_>) -> Result<(), Error> {
    deprecation_notice(ctx).await
}

/// The single static reply every subcommand shows (legacy ":warning: Caution!"
/// embed, error color, public like the legacy reply).
async fn deprecation_notice(ctx: Context<'_>) -> Result<(), Error> {
    let locale = util::locale(&ctx);

    let embed = serenity::CreateEmbed::new()
        .title(i18n::t(&locale, "permissions-deprecated-title"))
        .description(i18n::t(&locale, "permissions-deprecated-description"))
        .colour(util::COLOR_ERROR);

    util::reply_embed(ctx, embed, false).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n;

    #[test]
    fn permissions_registers_the_three_legacy_subcommands() {
        let command = permissions();
        let mut names: Vec<_> = command
            .subcommands
            .iter()
            .map(|c| c.name.to_string())
            .collect();
        names.sort();
        assert_eq!(names, ["add", "list", "remove"]);
        for subcommand in &command.subcommands {
            let description = subcommand
                .description
                .as_deref()
                .unwrap_or_else(|| panic!("/permissions {} has no description", subcommand.name));
            assert!(!description.is_empty() && description.len() <= 100);
        }
    }

    #[test]
    fn permissions_deprecation_catalog_points_to_native_permissions() {
        let locale = i18n::resolve(None);
        assert_ne!(
            i18n::t(&locale, "permissions-deprecated-title"),
            "permissions-deprecated-title"
        );
        let description = i18n::t(&locale, "permissions-deprecated-description");
        assert!(description.contains("deprecated"), "got: {description}");
        assert!(description.contains("Integrations"), "got: {description}");
        assert!(description.contains("discord.gg/kNmgU44xgU"), "got: {description}");
    }
}
