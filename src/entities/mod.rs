//! SeaORM entities for the normalized schema. One module per table,
//! matching `docs/specs/schema.md` exactly.

pub mod bot_stats;
pub mod guild_settings;
pub mod guilds;
pub mod leaderboard_panels;
pub mod role_rewards;
pub mod trophies;
pub mod user_trophies;

pub use bot_stats::Entity as BotStats;
pub use guild_settings::Entity as GuildSettings;
pub use guilds::Entity as Guilds;
pub use leaderboard_panels::Entity as LeaderboardPanels;
pub use role_rewards::Entity as RoleRewards;
pub use trophies::Entity as Trophies;
pub use user_trophies::Entity as UserTrophies;
