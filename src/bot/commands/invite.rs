//! `/invite` — shows the bot's OAuth2 invite link
//! (spec: docs/specs/commands-utility.md).
//!
//! Fixes vs legacy: the URL is built from the running application's ID
//! instead of a hardcoded client ID, and the reply is genuinely ephemeral
//! (the legacy dispatcher's public defer made `ephemeral: true` a no-op).

use poise::serenity_prelude as serenity;

use crate::bot::{Context, Error, util};
use crate::i18n;

/// Legacy permission bits requested on invite (Send Messages + Attach Files).
const INVITE_PERMISSIONS: u64 = 34816;

/// Builds the OAuth2 invite URL for the given application/client ID.
pub fn invite_url(client_id: u64) -> String {
    format!(
        "https://discord.com/oauth2/authorize?client_id={client_id}&permissions={INVITE_PERMISSIONS}&scope=applications.commands%20bot"
    )
}

/// Invite the bot to your server!
#[poise::command(slash_command)]
pub async fn invite(ctx: Context<'_>) -> Result<(), Error> {
    let locale = util::locale(&ctx);
    let (client_id, avatar) = {
        let current_user = ctx.cache().current_user();
        (current_user.id.get(), current_user.face())
    };

    let embed = serenity::CreateEmbed::new()
        .title(i18n::t(&locale, "invite-title"))
        .description(i18n::t_args(
            &locale,
            "invite-description",
            &[("url", invite_url(client_id).into())],
        ))
        .thumbnail(avatar)
        .colour(util::COLOR_MAIN);

    util::reply_embed(ctx, embed, true).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n;

    #[test]
    fn invite_url_contains_client_id_permissions_and_scopes() {
        let url = invite_url(985134052665356299);
        assert_eq!(
            url,
            "https://discord.com/oauth2/authorize?client_id=985134052665356299&permissions=34816&scope=applications.commands%20bot"
        );
    }

    #[test]
    fn invite_catalog_renders_url() {
        let locale = i18n::resolve(None);
        assert_ne!(i18n::t(&locale, "invite-title"), "invite-title");
        let url = invite_url(42);
        let description =
            i18n::t_args(&locale, "invite-description", &[("url", url.clone().into())]);
        assert!(description.contains(&url), "got: {description}");
    }
}
