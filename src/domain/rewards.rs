//! Role-reward target computation (commands-admin.md `/rewards`,
//! core-behaviors.md `doRewardRoles`). Pure function: the Rust reward engine
//! computes ONE final target role set per user, eliminating the legacy
//! add-then-remove ordering hazard where a role in both lists ended up
//! removed.

/// How reward roles combine, from the `stack_roles` guild setting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StackMode {
    /// `stack_roles == 0`: the user holds every reward whose requirement is met.
    All,
    /// `stack_roles == 1` (default): only the single highest-requirement met reward.
    HighestOnly,
}

impl StackMode {
    /// Map the stored `stack_roles` setting value to a mode (0 = All,
    /// anything else = HighestOnly, matching the legacy `?? 1` semantics).
    pub fn from_setting(value: i16) -> Self {
        if value == 0 {
            StackMode::All
        } else {
            StackMode::HighestOnly
        }
    }
}

/// The exact set of reward roles a user with `score` should hold.
///
/// `rewards` is a slice of `(role_id, requirement)` pairs in any order.
/// A reward is "met" when `requirement <= score`. Under [`StackMode::All`]
/// every met reward is returned (in input order); under
/// [`StackMode::HighestOnly`] only the single met reward with the highest
/// requirement is returned (first one in input order on a tie). No reward
/// met — including any negative score, since requirements are >= 1 — yields
/// an empty vec, meaning "remove all reward roles".
pub fn target_roles(score: i64, rewards: &[(i64, i64)], stack: StackMode) -> Vec<i64> {
    let met = rewards.iter().filter(|&&(_, requirement)| requirement <= score);
    match stack {
        StackMode::All => met.map(|&(role_id, _)| role_id).collect(),
        StackMode::HighestOnly => met
            // Strict `>` keeps the FIRST met reward in input order on a tie.
            .fold(None::<(i64, i64)>, |best, &(role_id, requirement)| {
                match best {
                    Some((_, best_requirement)) if requirement > best_requirement => {
                        Some((role_id, requirement))
                    }
                    None => Some((role_id, requirement)),
                    _ => best,
                }
            })
            .map(|(role_id, _)| vec![role_id])
            .unwrap_or_default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Unsorted on purpose: the function must not assume sorted input.
    const REWARDS: [(i64, i64); 4] = [(200, 50), (100, 10), (400, 1000), (300, 100)];

    #[test]
    fn empty_rewards_yield_no_roles_in_both_modes() {
        assert!(target_roles(1_000_000, &[], StackMode::All).is_empty());
        assert!(target_roles(1_000_000, &[], StackMode::HighestOnly).is_empty());
    }

    #[test]
    fn stack_all_returns_every_met_reward() {
        assert_eq!(
            target_roles(100, &REWARDS, StackMode::All),
            vec![200, 100, 300],
            "requirements 50, 10 and 100 are met; 1000 is not"
        );
    }

    #[test]
    fn highest_only_returns_single_highest_met_reward() {
        assert_eq!(
            target_roles(100, &REWARDS, StackMode::HighestOnly),
            vec![300],
            "only the highest met requirement (100) wins"
        );
    }

    #[test]
    fn requirement_equal_to_score_is_met() {
        let rewards = [(1, 10)];
        assert_eq!(target_roles(10, &rewards, StackMode::All), vec![1]);
        assert_eq!(target_roles(10, &rewards, StackMode::HighestOnly), vec![1]);
        assert!(target_roles(9, &rewards, StackMode::All).is_empty());
    }

    #[test]
    fn negative_score_meets_nothing() {
        assert!(target_roles(-5, &REWARDS, StackMode::All).is_empty());
        assert!(target_roles(-5, &REWARDS, StackMode::HighestOnly).is_empty());
    }

    #[test]
    fn zero_score_meets_nothing_since_requirements_are_at_least_one() {
        assert!(target_roles(0, &REWARDS, StackMode::All).is_empty());
        assert!(target_roles(0, &REWARDS, StackMode::HighestOnly).is_empty());
    }

    #[test]
    fn highest_only_tie_keeps_first_in_input_order() {
        // Duplicate requirements are rejected app-side on add, but legacy
        // imports could theoretically carry them: be deterministic anyway.
        let rewards = [(7, 10), (8, 10)];
        assert_eq!(target_roles(50, &rewards, StackMode::HighestOnly), vec![7]);
    }

    #[test]
    fn stack_mode_from_setting_maps_zero_to_all_else_highest() {
        assert_eq!(StackMode::from_setting(0), StackMode::All);
        assert_eq!(StackMode::from_setting(1), StackMode::HighestOnly);
        // Legacy read was `?? 1`: any unexpected value behaves as default.
        assert_eq!(StackMode::from_setting(2), StackMode::HighestOnly);
    }
}
