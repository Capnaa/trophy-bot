//! Trophy name normalization and legacy dedupe rules (ADR 0005).
//! Implemented with TDD during Phase 1.

/// Maximum trophy name length in characters (legacy limit, kept in the new schema).
const MAX_NAME_CHARS: usize = 32;

/// Normalize a trophy name per ADR 0005.
///
/// Unicode-aware: lowercases and keeps only alphanumeric characters of any
/// script (`char::is_alphanumeric`). If nothing survives (emoji/symbol-only
/// names), falls back to the lowercased raw name — the exact rule documented
/// in ADR 0005 / schema.md — so distinct emoji names stay distinct while
/// exact emoji duplicates still collide.
pub fn normalize_name(name: &str) -> String {
    let normalized: String = name
        .chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect();
    if normalized.is_empty() {
        name.to_lowercase()
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
/// normalized name) with any other final name in the guild, further
/// disambiguators are appended (`"{name} {legacy_id} {n}"` for n = 2, 3, …)
/// until unique — each attempt is rebuilt from the original name, so
/// truncation can never produce the same candidate twice in a row (the
/// growing counter guarantees progress even when the base is fully
/// truncated away). Processing follows input order, so the plan is
/// deterministic. Size-1 groups are untouched.
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
        let mut attempt = 1usize;
        let mut new_name = disambiguate(name, legacy_id, attempt);
        while taken.contains(&normalize_name(&new_name)) {
            attempt += 1;
            new_name = disambiguate(name, legacy_id, attempt);
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

/// Build the `attempt`-th rename candidate for `base` / `id`, always fitting
/// within [`MAX_NAME_CHARS`] characters.
///
/// Attempt 1 appends `" {id}"`; attempt n > 1 appends `" {id} {n}"`. The
/// suffix is preserved verbatim and only `base` is truncated (in characters)
/// to make room, so candidates for different attempts have different
/// normalized keys once the base is exhausted (the counter digits differ),
/// which guarantees the caller's collision loop terminates.
fn disambiguate(base: &str, id: &str, attempt: usize) -> String {
    let suffix = if attempt == 1 {
        format!(" {id}")
    } else {
        format!(" {id} {attempt}")
    };
    let max_base = MAX_NAME_CHARS.saturating_sub(suffix.chars().count());
    let truncated: String = base.chars().take(max_base).collect();
    let truncated = truncated.trim_end();
    if truncated.is_empty() {
        suffix.trim_start().to_string()
    } else {
        format!("{truncated}{suffix}")
    }
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
    fn normalize_emoji_only_falls_back_to_lowercased_raw() {
        // ADR 0005 / schema.md: the fallback key is the lowercased raw name,
        // untrimmed — surrounding whitespace is part of the key.
        assert_eq!(normalize_name("🏆"), "🏆");
        assert_eq!(normalize_name(" 🏆 "), " 🏆 ");
        assert_ne!(normalize_name(" 🏆 "), normalize_name("🏆"));
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
        // "test 3" again, so a further disambiguator is appended until unique.
        let renames = plan_renames(&input(&[
            ("3", "test"),
            ("7", "test"),
            ("8", "test 3"),
        ]));
        let ids: Vec<&str> = renames.iter().map(|r| r.legacy_id.as_str()).collect();
        assert_eq!(ids, vec!["3", "7"]);
        assert_eq!(renames[0].new_name, "test 3 2");
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
    fn truncated_rename_avoids_collision_with_existing_final_name() {
        // The 32-char base forces truncation when appending the id; the
        // truncated candidate ("Combat Meritorious Service Me 35") already
        // exists as a unique (untouched) trophy name. Previously this hung
        // forever: truncation removed exactly the " 35" that was then
        // re-appended, so the candidate never changed.
        let name = "Combat Meritorious Service Medal";
        assert_eq!(name.chars().count(), 32);
        let renames = plan_renames(&input(&[
            ("35", name),
            ("38", name),
            ("8", "Combat Meritorious Service Me 35"),
        ]));
        let ids: Vec<&str> = renames.iter().map(|r| r.legacy_id.as_str()).collect();
        assert_eq!(ids, vec!["35", "38"]);
        // All final names fit the limit and are unique by normalized key,
        // including against the untouched pre-existing name.
        let mut keys: Vec<String> = renames
            .iter()
            .map(|r| {
                assert!(
                    r.new_name.chars().count() <= MAX_NAME_CHARS,
                    "too long: {}",
                    r.new_name
                );
                normalize_name(&r.new_name)
            })
            .chain(std::iter::once(normalize_name(
                "Combat Meritorious Service Me 35",
            )))
            .collect();
        keys.sort();
        keys.dedup();
        assert_eq!(keys.len(), 3);
    }

    #[test]
    fn disambiguation_progresses_even_when_base_is_fully_truncated() {
        // Adversarial guild: every early candidate for renaming id 1 is
        // pre-taken by unique trophies, forcing many attempts. The loop must
        // still terminate and produce a unique, <=32-char name.
        let mut pairs: Vec<(String, String)> = vec![
            ("1".to_string(), "x".to_string()),
            ("2".to_string(), "x".to_string()),
        ];
        pairs.push(("3".to_string(), "x 1".to_string())); // blocks attempt 1
        for n in 2..=9 {
            // Blocks attempts 2..=9 ("x 1 {n}").
            pairs.push((format!("{}", n + 2), format!("x 1 {n}")));
        }
        let renames = plan_renames(&pairs);
        assert_eq!(renames.len(), 2);
        let r1 = &renames[0];
        assert_eq!(r1.legacy_id, "1");
        assert_eq!(r1.new_name, "x 1 10");
        assert!(r1.new_name.chars().count() <= MAX_NAME_CHARS);
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
            ("8", "Combat Meritorious Service Me 35"),
            ("3", "test"),
            ("7", "test"),
            ("9", "test 3"),
            ("1", "🏆"),
            ("2", "🏆"),
        ]);
        let first = plan_renames(&trophies);
        for _ in 0..10 {
            assert_eq!(plan_renames(&trophies), first);
        }
    }
}
