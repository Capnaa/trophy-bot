//! SeaORM entities for the normalized schema. One module per table,
//! matching `docs/specs/schema.md` exactly.
//!
//! Consumers import the lowercase modules and use `<module>::Entity`
//! (e.g. `use crate::entities::trophies;` … `trophies::Entity::find()`);
//! there are deliberately no `Entity as CamelCase` re-export aliases so
//! only one import style exists.

pub mod active_medals_panels;
pub mod bot_stats;
pub mod guild_links;
pub mod guild_settings;
pub mod guilds;
pub mod leaderboard_panels;
pub mod medals_overview_panels;
pub mod retired_medals_overview_panels;
pub mod role_rewards;
pub mod trophies;
pub mod user_trophies;

#[cfg(test)]
mod tests {
    use sea_orm::EntityName;

    /// Every entity module maps to its `docs/specs/schema.md` table name.
    /// Also anchors the single supported import style (`<module>::Entity`).
    #[test]
    fn entity_table_names_match_schema() {
        use super::*;
        assert_eq!(active_medals_panels::Entity.table_name(), "active_medals_panels");
        assert_eq!(bot_stats::Entity.table_name(), "bot_stats");
        assert_eq!(guild_links::Entity.table_name(), "guild_links");
        assert_eq!(guild_settings::Entity.table_name(), "guild_settings");
        assert_eq!(guilds::Entity.table_name(), "guilds");
        assert_eq!(leaderboard_panels::Entity.table_name(), "leaderboard_panels");
        assert_eq!(medals_overview_panels::Entity.table_name(), "medals_overview_panels");
        assert_eq!(
            retired_medals_overview_panels::Entity.table_name(),
            "retired_medals_overview_panels"
        );
        assert_eq!(role_rewards::Entity.table_name(), "role_rewards");
        assert_eq!(trophies::Entity.table_name(), "trophies");
        assert_eq!(user_trophies::Entity.table_name(), "user_trophies");
    }
}
