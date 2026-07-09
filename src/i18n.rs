//! i18n layer (ADR 0010): Fluent catalogs under `locales/`, locale taken from
//! the interaction (`ctx.locale()`) or `guild.preferred_locale` for
//! non-interaction output. Resolution chain: exact tag → language prefix → en-US.

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::LazyLock;

use fluent_templates::{Loader, static_loader};

pub use fluent_templates::LanguageIdentifier;
pub use fluent_templates::fluent_bundle::FluentValue;

static_loader! {
    static LOCALES = {
        locales: "./locales",
        fallback_language: "en-US",
    };
}

static FALLBACK: LazyLock<LanguageIdentifier> =
    LazyLock::new(|| "en-US".parse().expect("valid fallback language tag"));

/// Resolves a Discord locale tag (e.g. `es-ES`, `pt-BR`) to an available
/// catalog: exact match → same primary language → `en-US`.
pub fn resolve(locale: Option<&str>) -> LanguageIdentifier {
    let Some(tag) = locale else {
        return FALLBACK.clone();
    };
    let Ok(wanted) = tag.parse::<LanguageIdentifier>() else {
        return FALLBACK.clone();
    };

    let mut language_match = None;
    for available in LOCALES.locales() {
        if *available == wanted {
            return wanted;
        }
        if available.language == wanted.language && language_match.is_none() {
            language_match = Some(available.clone());
        }
    }
    language_match.unwrap_or_else(|| FALLBACK.clone())
}

/// Looks up a message. Missing keys return the key itself (never panics).
pub fn t(locale: &LanguageIdentifier, key: &str) -> String {
    LOCALES
        .try_lookup(locale, key)
        .unwrap_or_else(|| key.to_string())
}

/// Looks up a message with arguments.
pub fn t_args(
    locale: &LanguageIdentifier,
    key: &str,
    args: &[(&'static str, FluentValue<'static>)],
) -> String {
    let map: HashMap<Cow<'static, str>, FluentValue<'static>> = args
        .iter()
        .map(|(k, v)| (Cow::Borrowed(*k), v.clone()))
        .collect();
    LOCALES
        .try_lookup_with_args(locale, key, &map)
        .unwrap_or_else(|| key.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_exact_locale() {
        assert_eq!(resolve(Some("en-US")).to_string(), "en-US");
    }

    #[test]
    fn falls_back_to_english_for_unknown_locale() {
        assert_eq!(resolve(Some("zz-ZZ")).to_string(), "en-US");
        assert_eq!(resolve(Some("not a tag !!")).to_string(), "en-US");
        assert_eq!(resolve(None).to_string(), "en-US");
    }

    #[test]
    fn looks_up_message_with_args() {
        let locale = resolve(None);
        let text = t_args(
            &locale,
            "award-error-count",
            &[("min", 1.into()), ("max", 50.into())],
        );
        assert!(text.contains('1') && text.contains("50"), "got: {text}");
    }

    #[test]
    fn missing_key_returns_key() {
        assert_eq!(t(&resolve(None), "no-such-key"), "no-such-key");
    }

    #[test]
    fn plural_selection_works() {
        let locale = resolve(None);
        let one = t_args(&locale, "common-error-cooldown", &[("seconds", 1.into())]);
        let many = t_args(&locale, "common-error-cooldown", &[("seconds", 3.into())]);
        assert!(!one.contains("seconds"), "got: {one}");
        assert!(many.contains('3') && many.contains("seconds"), "got: {many}");
    }
}
