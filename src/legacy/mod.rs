use std::collections::HashMap;
use sea_orm::{ConnectionTrait, Database, DatabaseConnection};
use sea_orm::sea_query::Query;

#[derive(Debug)]
pub struct LegacyData {
    pub bot: serde_json::Value,
    pub guilds: serde_json::Value,
}

impl LegacyData {
    pub async fn load() -> Option<Self> {
        let bot = get_json("bot").await?;
        let guilds = get_json("guilds").await?;

        Some(Self { bot, guilds })
    }

    pub fn guilds(&self) -> Vec<&serde_json::Value> {
        self.guilds.as_object()
            .map(|obj| obj.values().collect())
            .unwrap_or_default()
    }

    pub fn bot_stats(&self) -> HashMap<String, u64> {
        let mut map = HashMap::new();
        if let Some(commands) = self.bot.get("commands").and_then(|v| v.as_object()) {
            for (key, value) in commands {
                if let Some(num) = value.as_u64() {
                    map.insert(key.clone(), num);
                }
            }
        }
        if let Some(trophies_awarded) = self.bot.get("trophiesAwarded").and_then(|v| v.as_u64()) {
            map.insert("trophiesAwarded".to_string(), trophies_awarded);
        }
        if let Some(trophies) = self.bot.get("trophies").and_then(|v| v.as_u64()) {
            map.insert("rootTrophies".to_string(), trophies);
        }
        if let Some(last_day) = self.bot.get("lastDay").and_then(|v| v.as_u64()) {
            map.insert("lastDay".to_string(), last_day);
        }
        map
    }
}

async fn get_json(table: &'static str) -> Option<serde_json::Value> {
    let db: DatabaseConnection = Database::connect("sqlite://./json.sqlite")
        .await
        .ok()?;

    let mut query = Query::select();
    query
        .from(table)
        .column("json")
        .limit(1);

    let column: String = db
        .query_one(&query)
        .await
        .ok()?
        ?
        .try_get("", "json")
        .ok()?;

    db.close().await.ok()?;

    serde_json::from_str(&column).ok()?
}
