//! Trophy name normalization and legacy dedupe rules (ADR 0005).
//! Implemented with TDD during Phase 1.

/// Maximum trophy name length in characters (legacy limit, kept in the new schema).
const MAX_NAME_CHARS: usize = 32;

/// Normalize a trophy name per ADR 0005.
///
/// Unicode-aware: lowercases and keeps only alphanumeric characters of any
/// script (`char::is_alphanumeric`). If nothing survives (emoji/symbol-only
/// names), falls back to the trimmed whole name, lowercased, so distinct
/// emoji names stay distinct while exact emoji duplicates still collide.
pub fn normalize_name(name: &str) -> String {
    let normalized: String = name
        .chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect();
    if normalized.is_empty() {
        name.trim().to_lowercase()
    } else {
        normalized
    }
}

/// A planned rename of one legacy trophy, recorded in the import report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rename {
    pub legacy_id: String,
    pub old_name: String,
    pub new_name: String,
}

/// Plan the dedupe renames for one guild's trophies (ADR 0005 importer rule).
///
/// Input: `(legacy_id, name)` pairs for a single guild. Trophies whose
/// normalized name is shared by more than one trophy are renamed to
/// `"{name} {legacy_id}"`; the base name is truncated so the result fits
/// [`MAX_NAME_CHARS`] characters. If a candidate still collides (by
/// normalized name) with any other final name in the guild, the legacy id
/// is appended again until unique. Processing follows input order, so the
/// plan is deterministic. Size-1 groups are untouched.
pub fn plan_renames(trophies: &[(String, String)]) -> Vec<Rename> {
    use std::collections::{HashMap, HashSet};

    // Count how many trophies share each normalized name.
    let mut counts: HashMap<String, usize> = HashMap::new();
    for (_, name) in trophies {
        *counts.entry(normalize_name(name)).or_insert(0) += 1;
    }

    // Final names already taken: every trophy that is NOT being renamed.
    let mut taken: HashSet<String> = trophies
        .iter()
        .map(|(_, name)| normalize_name(name))
        .filter(|key| counts[key] == 1)
        .collect();

    // Rename duplicate-group members in input order (deterministic).
    let mut renames = Vec::new();
    for (legacy_id, name) in trophies {
        if counts[&normalize_name(name)] == 1 {
            continue;
        }
        let mut new_name = append_id(name, legacy_id);
        while taken.contains(&normalize_name(&new_name)) {
            new_name = append_id(&new_name, legacy_id);
        }
        taken.insert(normalize_name(&new_name));
        renames.push(Rename {
            legacy_id: legacy_id.clone(),
            old_name: name.clone(),
            new_name,
        });
    }
    renames
}

/// Append `" {id}"` to `base`, truncating `base` (in characters) so the
/// result fits within [`MAX_NAME_CHARS`].
fn append_id(base: &str, id: &str) -> String {
    let id_chars = id.chars().count();
    let max_base = MAX_NAME_CHARS.saturating_sub(id_chars + 1);
    let truncated: String = base.chars().take(max_base).collect();
    format!("{} {}", truncated.trim_end(), id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(id, name)| (id.to_string(), name.to_string()))
            .collect()
    }

    // --- normalize_name ---

    #[test]
    fn normalize_is_case_and_whitespace_insensitive() {
        assert_eq!(normalize_name("Test"), normalize_name("test"));
        assert_eq!(normalize_name("test"), normalize_name("test "));
    }

    #[test]
    fn normalize_strips_punctuation_and_spaces() {
        assert_eq!(
            normalize_name("Do You Smell Barbecue?"),
            "doyousmellbarbecue"
        );
    }

    #[test]
    fn normalize_keeps_cyrillic_lowercased() {
        assert_eq!(normalize_name("Новичок"), "новичок");
    }

    #[test]
    fn normalize_keeps_cjk() {
        assert_eq!(normalize_name("受"), "受");
    }

    #[test]
    fn normalize_emoji_only_falls_back_to_trimmed_lowercased_raw() {
        assert_eq!(normalize_name("🏆"), "🏆");
        assert_eq!(normalize_name(" 🏆 "), "🏆");
    }

    #[test]
    fn normalize_keeps_accented_letters() {
        assert_eq!(
            normalize_name("Magyar Vitézségi Érdemrend"),
            "magyarvitézségiérdemrend"
        );
    }

    // --- plan_renames ---

    #[test]
    fn pair_collision_renames_both_with_legacy_ids() {
        let renames = plan_renames(&input(&[("3", "test"), ("7", "test")]));
        assert_eq!(
            renames,
            vec![
                Rename {
                    legacy_id: "3".into(),
                    old_name: "test".into(),
                    new_name: "test 3".into(),
                },
                Rename {
                    legacy_id: "7".into(),
                    old_name: "test".into(),
                    new_name: "test 7".into(),
                },
            ]
        );
    }

    #[test]
    fn triple_collision_renames_all_three() {
        let renames = plan_renames(&input(&[
            ("1", "Do You Smell Barbecue?"),
            ("2", "Do You Smell Barbecue?"),
            ("9", "do you smell barbecue"),
        ]));
        let new_names: Vec<&str> = renames.iter().map(|r| r.new_name.as_str()).collect();
        assert_eq!(
            new_names,
            vec![
                "Do You Smell Barbecue? 1",
                "Do You Smell Barbecue? 2",
                "do you smell barbecue 9",
            ]
        );
    }

    #[test]
    fn overflow_truncates_base_so_result_fits_32_chars() {
        // Exactly 32 chars long, so " {id}" cannot fit without truncation.
        let name = "Combat Meritorious Service Medal";
        assert_eq!(name.chars().count(), 32);
        let renames = plan_renames(&input(&[("35", name), ("38", name)]));
        assert_eq!(renames.len(), 2);
        for r in &renames {
            assert!(r.new_name.chars().count() <= 32, "too long: {}", r.new_name);
        }
        assert_ne!(renames[0].new_name, renames[1].new_name);
        assert_ne!(
            normalize_name(&renames[0].new_name),
            normalize_name(&renames[1].new_name)
        );
        assert!(renames[0].new_name.ends_with(" 35"));
        assert!(renames[1].new_name.ends_with(" 38"));
    }

    #[test]
    fn emoji_duplicates_renamed_but_distinct_symbols_untouched() {
        let renames = plan_renames(&input(&[
            ("1", "🏆"),
            ("2", "🏆"),
            ("3", "🥇"),
            ("4", "受"),
        ]));
        let ids: Vec<&str> = renames.iter().map(|r| r.legacy_id.as_str()).collect();
        assert_eq!(ids, vec!["1", "2"]);
        assert_eq!(renames[0].new_name, "🏆 1");
        assert_eq!(renames[1].new_name, "🏆 2");
    }

    #[test]
    fn rename_avoids_collision_with_existing_final_name() {
        // "test 3" already exists untouched; the rename of id 3 would produce
        // "test 3" again, so the id must be appended again until unique.
        let renames = plan_renames(&input(&[
            ("3", "test"),
            ("7", "test"),
            ("8", "test 3"),
        ]));
        let ids: Vec<&str> = renames.iter().map(|r| r.legacy_id.as_str()).collect();
        assert_eq!(ids, vec!["3", "7"]);
        assert_eq!(renames[0].new_name, "test 3 3");
        assert_eq!(renames[1].new_name, "test 7");
        // All final names unique by normalized key.
        let mut keys: Vec<String> = renames
            .iter()
            .map(|r| normalize_name(&r.new_name))
            .chain(std::iter::once(normalize_name("test 3")))
            .collect();
        keys.sort();
        keys.dedup();
        assert_eq!(keys.len(), 3);
    }

    #[test]
    fn size_one_groups_are_untouched() {
        let renames = plan_renames(&input(&[("1", "Alpha"), ("2", "Beta"), ("3", "Gamma")]));
        assert!(renames.is_empty());
    }

    #[test]
    fn plan_is_deterministic() {
        let trophies = input(&[
            ("35", "Combat Meritorious Service Medal"),
            ("38", "Combat Meritorious Service Medal"),
            ("3", "test"),
            ("7", "test"),
            ("8", "test 3"),
            ("1", "🏆"),
            ("2", "🏆"),
        ]);
        let first = plan_renames(&trophies);
        for _ in 0..10 {
            assert_eq!(plan_renames(&trophies), first);
        }
    }
}
