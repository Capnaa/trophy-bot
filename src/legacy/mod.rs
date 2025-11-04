use sea_orm::{ConnectionTrait, Database, DatabaseConnection};
use sea_orm::sea_query::Query;

#[derive(Debug)]
pub struct LegacyData {
    pub bot: serde_json::Value,
    pub guilds: serde_json::Value,
}

impl LegacyData {
    pub async fn load() -> Self {
        let bot = get_json("bot").await;
        let guilds = get_json("guilds").await;

        Self { bot, guilds }
    }

    pub fn guild(&self, guild_id: u64) -> Option<&serde_json::Value> {
        self.guilds.get(guild_id.to_string())
    }

    pub fn guilds(&self) -> Vec<&serde_json::Value> {
        self.guilds.as_object()
            .map(|obj| obj.values().collect())
            .unwrap_or_default()
    }
}

async fn get_json(table: &'static str) -> serde_json::Value {
    let db: DatabaseConnection = Database::connect("sqlite://./json.sqlite")
        .await
        .expect("Failed to connect to database");

    let mut query = Query::select();
    query
        .from(table)
        .column("json")
        .limit(1);

    let column: String = db
        .query_one(&query)
        .await
        .expect("Failed to execute query")
        .expect("Failed to query row")
        .try_get("", "json")
        .expect("Failed to get json from row");

    db.close().await.expect("Failed to close database");

    serde_json::from_str(&column).expect("Failed to parse JSON")
}
