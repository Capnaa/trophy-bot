//! Import report (`docs/specs/migration-import.md` Phase 7).
//!
//! Every anomaly the importer encounters — skips, renames, roundings,
//! default-fills, drops, dedupes and mismatches — is recorded here, written to
//! JSON and summarized against the counts measured on the production snapshot.
//! Cutover proceeds only after human review of this report.

use serde::Serialize;

/// One trophy field that was absent in legacy and filled with a default.
#[derive(Debug, Clone, Serialize)]
pub struct DefaultedField {
    pub guild_id: i64,
    pub legacy_id: String,
    pub field: &'static str,
}

/// One legacy field whose value was PRESENT but unusable (non-numeric
/// snowflake, out-of-range timestamp, value/index/requirement outside its
/// schema CHECK range, ...); imported as NULL / clamped / dropped instead of
/// aborting the transaction. `legacy_id` names the trophy id, setting key or
/// reward role id. Defense path — 0 expected in production (spec principle 3:
/// anomalies are reported, never silently fixed).
#[derive(Debug, Clone, Serialize)]
pub struct InvalidFieldValue {
    pub guild_id: i64,
    pub legacy_id: String,
    pub field: &'static str,
    /// The unusable legacy value, verbatim.
    pub value: String,
}

/// One root `guilds` entry whose value is neither a guild object nor a
/// `/forgetme` tombstone; skipped, with the verbatim value kept so the
/// pre-cutover human review (migration-import.md Phase 0) can inspect it
/// without excavating the legacy blob.
#[derive(Debug, Clone, Serialize)]
pub struct CorruptGuild {
    pub key: String,
    /// The verbatim non-object, non-tombstone legacy value.
    pub value: serde_json::Value,
}

/// One non-integer legacy trophy value rounded half-away-from-zero.
#[derive(Debug, Clone, Serialize)]
pub struct RoundedValue {
    pub guild_id: i64,
    pub legacy_id: String,
    pub original: f64,
    pub rounded: i32,
}

/// One trophy renamed by the ADR 0005 dedupe plan.
#[derive(Debug, Clone, Serialize)]
pub struct RenamedTrophy {
    pub guild_id: i64,
    pub legacy_id: String,
    pub old_name: String,
    pub new_name: String,
}

/// One award array element referencing a nonexistent trophy (dropped).
#[derive(Debug, Clone, Serialize)]
pub struct OrphanedAward {
    pub guild_id: i64,
    pub user_id: i64,
    pub legacy_trophy_id: String,
}

/// One duplicate role-reward entry removed (the lowest requirement was kept).
#[derive(Debug, Clone, Serialize)]
pub struct DedupedReward {
    pub guild_id: i64,
    pub role_id: i64,
    pub kept_requirement: i32,
    pub removed_requirement: i32,
}

/// One referenced local image file missing from disk (image stored as NULL).
#[derive(Debug, Clone, Serialize)]
pub struct MissingImageFile {
    pub guild_id: i64,
    pub legacy_id: String,
    pub filename: String,
}

/// One CDN image URL that could not be downloaded (image stored as NULL).
#[derive(Debug, Clone, Serialize)]
pub struct ExpiredImageUrl {
    pub guild_id: i64,
    pub legacy_id: String,
    pub url: String,
}

/// One CDN image URL successfully downloaded into `images/`.
#[derive(Debug, Clone, Serialize)]
pub struct DownloadedImage {
    pub guild_id: i64,
    pub legacy_id: String,
    pub url: String,
    pub filename: String,
}

/// Why a user's stored `trophyValue` disagrees with the recalculated sum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MismatchKind {
    /// Disagrees even with the raw float values: genuine legacy drift.
    LegacyDrift,
    /// Matches the raw float sum; induced by rounding float trophy values.
    Rounding,
}

/// One user whose legacy `trophyValue` differs from the recalculated score.
/// NOT reconciled (ADR 0006): the recalculated value is correct by definition.
#[derive(Debug, Clone, Serialize)]
pub struct ScoreMismatch {
    pub guild_id: i64,
    pub user_id: i64,
    /// Legacy stored `trophyValue` (floats exist).
    pub stored: f64,
    /// Sum of the ROUNDED as-stored trophy values (orphans excluded).
    pub recalculated: i64,
    /// Sum of the raw legacy float values, used for classification.
    pub raw_recalculated: f64,
    pub kind: MismatchKind,
}

/// Full outcome of one import run (all counters and anomaly lists).
#[derive(Debug, Default, Serialize)]
pub struct ImportReport {
    // Phase 0
    pub tombstoned_guilds: Vec<String>,
    pub corrupt_guilds: Vec<CorruptGuild>,
    // Phase 1
    pub bot_stats_rows: u64,
    // Phase 2
    pub guilds: u64,
    // Phase 3
    pub trophies: u64,
    pub defaulted_fields: Vec<DefaultedField>,
    pub invalid_fields: Vec<InvalidFieldValue>,
    pub rounded_values: Vec<RoundedValue>,
    pub renamed_trophies: Vec<RenamedTrophy>,
    // Phase 4
    pub awards_inserted: u64,
    pub orphaned_awards: Vec<OrphanedAward>,
    pub users_with_awards: u64,
    pub empty_award_users: u64,
    // Phase 5
    pub role_rewards: u64,
    pub deduped_rewards: Vec<DedupedReward>,
    pub panels: u64,
    pub settings_rows: u64,
    // Phase 6
    pub local_images_kept: u64,
    pub missing_image_files: Vec<MissingImageFile>,
    pub downloaded_images: Vec<DownloadedImage>,
    pub expired_image_urls: Vec<ExpiredImageUrl>,
    pub orphan_disk_files: Vec<String>,
    // Phase 7
    pub score_mismatches: Vec<ScoreMismatch>,
}

impl ImportReport {
    pub fn drift_mismatches(&self) -> u64 {
        self.score_mismatches.iter().filter(|m| m.kind == MismatchKind::LegacyDrift).count() as u64
    }

    pub fn rounding_mismatches(&self) -> u64 {
        self.score_mismatches.iter().filter(|m| m.kind == MismatchKind::Rounding).count() as u64
    }

    /// Total URL-shaped images handled (downloaded + expired).
    pub fn url_images(&self) -> u64 {
        (self.downloaded_images.len() + self.expired_image_urls.len()) as u64
    }

    /// Distinct trophies with a defaulted CORE field (`creator`, `created`,
    /// `signed`) — the spec's "43 incomplete trophies" from the pre-rewrite
    /// era (migration-import.md Phase 3 / data-model-legacy.md). The
    /// `details`/`description`/`emoji` defaults are recorded per-field too
    /// but tracked separately: missing `details` alone is expected legacy
    /// shape (360 trophies), not pre-rewrite incompleteness.
    pub fn defaulted_trophies(&self) -> u64 {
        self.defaulted_fields
            .iter()
            .filter(|d| matches!(d.field, "creator" | "created" | "signed"))
            .map(|d| (d.guild_id, d.legacy_id.as_str()))
            .collect::<std::collections::HashSet<_>>()
            .len() as u64
    }

    /// Trophies whose `details` field was absent and filled with the default.
    pub fn defaulted_details(&self) -> u64 {
        self.defaulted_fields.iter().filter(|d| d.field == "details").count() as u64
    }

    /// `(metric, measured, expected)` rows; expected values are the counts
    /// measured against the production snapshot (migration-import.md Phase 7).
    pub fn summary_rows(&self) -> Vec<(&'static str, u64, u64)> {
        vec![
            ("guilds", self.guilds, 2_488),
            ("tombstoned_guilds", self.tombstoned_guilds.len() as u64, 5),
            ("corrupt_guilds", self.corrupt_guilds.len() as u64, 0),
            ("trophies", self.trophies, 10_853),
            ("defaulted_trophies", self.defaulted_trophies(), 43),
            ("defaulted_details", self.defaulted_details(), 360),
            ("invalid_field_values", self.invalid_fields.len() as u64, 0),
            ("rounded_values", self.rounded_values.len() as u64, 44),
            ("renamed_trophies", self.renamed_trophies.len() as u64, 643),
            ("awards_inserted", self.awards_inserted, 60_554),
            ("orphaned_awards", self.orphaned_awards.len() as u64, 0),
            ("empty_award_users", self.empty_award_users, 1_284),
            ("role_rewards_after_dedupe", self.role_rewards, 275),
            ("deduped_rewards_removed", self.deduped_rewards.len() as u64, 12),
            ("panels", self.panels, 461),
            ("settings_rows", self.settings_rows, 162),
            ("score_mismatches", self.score_mismatches.len() as u64, 133),
            ("score_mismatches_legacy_drift", self.drift_mismatches(), 51),
            ("score_mismatches_rounding", self.rounding_mismatches(), 82),
            ("local_images_kept", self.local_images_kept, 2_493),
            ("missing_image_files", self.missing_image_files.len() as u64, 200),
            ("url_images", self.url_images(), 195),
            ("orphan_disk_files", self.orphan_disk_files.len() as u64, 278),
        ]
    }

    /// Logs the measured-vs-expected summary table.
    pub fn log_summary(&self) {
        log::info!(
            "{:<32} {:>10} {:>10}  {}",
            "metric",
            "measured",
            "expected",
            "status"
        );
        for (name, measured, expected) in self.summary_rows() {
            let status = if measured == expected { "OK" } else { "MISMATCH" };
            log::info!("{name:<32} {measured:>10} {expected:>10}  {status}");
        }
    }
}
