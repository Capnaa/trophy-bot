//! Typed model of the legacy quick.db JSON documents.
//!
//! Shapes, field presence and anomalies are documented in
//! `docs/specs/data-model-legacy.md`. All structs tolerate unknown keys
//! (no `deny_unknown_fields`): production carries vestigial fields such as
//! `id`, `language`, `restapi` and the one-off typo key `tropies`.

use serde::{Deserialize, Deserializer};
use std::collections::HashMap;

/// Root of the `bot` table JSON document. Counters are cumulative-only and
/// unreliable (never decremented); they are kept as a historical record.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct LegacyBot {
    /// Per-command run counters plus the aggregate `total` key.
    #[serde(default)]
    pub commands: HashMap<String, u64>,
    /// Cumulative "trophies created" counter (real count differs).
    #[serde(default)]
    pub trophies: u64,
    /// Cumulative "trophies awarded" counter (real count differs).
    #[serde(default, rename = "trophiesAwarded")]
    pub trophies_awarded: u64,
}

/// One root entry of the `guilds` document: a real guild object, a
/// `/forgetme` tombstone (the literal JSON integer `-1`), or any other
/// non-object value, which migration-import.md Phase 0 requires the importer
/// to skip and report as corrupt (0 expected in production).
#[derive(Debug, Clone)]
pub enum GuildEntry {
    Tombstone,
    Guild(Box<LegacyGuild>),
    /// Non-object, non-tombstone value, kept verbatim for the import report.
    Corrupt(serde_json::Value),
}

impl GuildEntry {
    pub fn is_tombstone(&self) -> bool {
        matches!(self, Self::Tombstone)
    }

    pub fn is_corrupt(&self) -> bool {
        matches!(self, Self::Corrupt(_))
    }

    pub fn as_guild(&self) -> Option<&LegacyGuild> {
        match self {
            Self::Guild(guild) => Some(guild),
            Self::Tombstone | Self::Corrupt(_) => None,
        }
    }
}

impl<'de> Deserialize<'de> for GuildEntry {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = serde_json::Value::deserialize(deserializer)?;
        if value.is_object() {
            return serde_json::from_value(value)
                .map(|guild| Self::Guild(Box::new(guild)))
                .map_err(serde::de::Error::custom);
        }
        if value.as_i64() == Some(-1) {
            return Ok(Self::Tombstone);
        }
        Ok(Self::Corrupt(value))
    }
}

/// A guild object from the legacy `guilds` document.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct LegacyGuild {
    /// Present in 2,407/2,488 guilds and always `true`; absence means `false`.
    pub imsafe: Option<bool>,
    /// Setting-id → option index. Missing keys use the legacy defaults.
    #[serde(default)]
    pub settings: HashMap<String, i64>,
    /// Trophy-id → definition, plus the special `"current"` counter key.
    #[serde(default, deserialize_with = "trophy_map")]
    pub trophies: HashMap<String, TrophyEntry>,
    /// User snowflake → award record.
    #[serde(default)]
    pub users: HashMap<String, LegacyUser>,
    #[serde(default)]
    pub rewards: Vec<LegacyReward>,
    pub panel: Option<LegacyPanel>,
}

impl LegacyGuild {
    /// Trophy definitions only, skipping the `"current"` next-id counter.
    pub fn trophy_defs(&self) -> impl Iterator<Item = (&str, &LegacyTrophy)> {
        self.trophies.iter().filter_map(|(id, entry)| match entry {
            TrophyEntry::Trophy(trophy) => Some((id.as_str(), trophy.as_ref())),
            TrophyEntry::Counter(_) => None,
        })
    }

    /// The `"current"` next-id counter, if the guild has one.
    pub fn trophy_counter(&self) -> Option<i64> {
        self.trophies.values().find_map(|entry| match entry {
            TrophyEntry::Counter(counter) => Some(*counter),
            TrophyEntry::Trophy(_) => None,
        })
    }
}

/// Value of the guild `trophies` map: every key is a trophy definition except
/// `"current"`, which holds the next-id counter as a bare integer.
#[derive(Debug, Clone)]
pub enum TrophyEntry {
    Counter(i64),
    Trophy(Box<LegacyTrophy>),
}

// Hand-rolled instead of `#[serde(untagged)]` so a malformed trophy surfaces
// its real serde cause (e.g. "missing field `name`") rather than the opaque
// "data did not match any variant" — vital when locating one bad trophy among
// the ~10,853 in the multi-megabyte production guilds document.
impl<'de> Deserialize<'de> for TrophyEntry {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = serde_json::Value::deserialize(deserializer)?;
        if let Some(counter) = value.as_i64() {
            return Ok(Self::Counter(counter));
        }
        serde_json::from_value(value)
            .map(|trophy| Self::Trophy(Box::new(trophy)))
            .map_err(serde::de::Error::custom)
    }
}

/// Deserializes the guild `trophies` map entry-by-entry so a parse failure
/// names the offending trophy id.
fn trophy_map<'de, D>(deserializer: D) -> Result<HashMap<String, TrophyEntry>, D::Error>
where
    D: Deserializer<'de>,
{
    let raw = HashMap::<String, serde_json::Value>::deserialize(deserializer)?;
    raw.into_iter()
        .map(|(id, value)| match serde_json::from_value(value) {
            Ok(entry) => Ok((id, entry)),
            Err(err) => Err(serde::de::Error::custom(format!("trophy `{id}`: {err}"))),
        })
        .collect()
}

/// A trophy definition. 43 pre-rewrite trophies miss `creator`/`created`/
/// `signed`/`details`; 44 values are non-integer floats.
#[derive(Debug, Clone, Deserialize)]
pub struct LegacyTrophy {
    pub name: String,
    pub description: Option<String>,
    pub emoji: Option<String>,
    /// Always present; kept as `f64` because 44 float values exist.
    pub value: f64,
    /// Creator user snowflake as a string.
    pub creator: Option<String>,
    /// Unix **milliseconds** timestamp.
    pub created: Option<i64>,
    pub signed: Option<bool>,
    pub details: Option<String>,
    /// `null`, local filename `{guild}_{id}.{ext}`, or a full CDN URL.
    pub image: Option<String>,
    /// Tolerates all four legacy shapes: absent, `{}`, explicit nulls,
    /// text-only, and user+name.
    #[serde(default, deserialize_with = "null_as_default")]
    pub dedication: LegacyDedication,
}

/// Trophy dedication; both fields are independently nullable.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct LegacyDedication {
    /// Dedicated user snowflake (always numeric when present).
    #[serde(default)]
    pub user: Option<String>,
    /// Free dedication text or the dedicated user's name.
    #[serde(default)]
    pub name: Option<String>,
}

/// Per-user award record. Each array element is one award (duplicates are
/// multiple awards of the same trophy); all elements are trophy-id strings.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct LegacyUser {
    #[serde(default)]
    pub trophies: Vec<String>,
    /// Denormalized score; floats and drift exist, so only useful for
    /// float-tolerant validation reports.
    #[serde(default, rename = "trophyValue")]
    pub trophy_value: f64,
}

/// Role reward rule. 7 guilds repeat the same role id (legacy duplicate bug).
#[derive(Debug, Clone, Deserialize)]
pub struct LegacyReward {
    pub role: String,
    pub requirement: i64,
}

/// Persistent leaderboard panel target.
#[derive(Debug, Clone, Deserialize)]
pub struct LegacyPanel {
    pub message: String,
    pub channel: String,
}

/// Deserializes an explicit JSON `null` as `T::default()`.
fn null_as_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Default + Deserialize<'de>,
{
    Ok(Option::<T>::deserialize(deserializer)?.unwrap_or_default())
}
