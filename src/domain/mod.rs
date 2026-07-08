//! Shared domain layer: logic every command and the importer reuse.
//! See `docs/specs/implementation-plan.md` Phase 1.

pub mod normalize;
pub mod queries;
pub mod rewards;
pub mod settings;

#[cfg(test)]
pub(crate) mod test_support;
