//! Host-locale resolution for desktop-entry localized keys.
//!
//! Mirrors the freedesktop.org desktop-entry spec's lookup for `LC_MESSAGES`:
//! try `lang_COUNTRY@modifier`, then `lang_COUNTRY`, then `lang@modifier`,
//! then `lang`, then the unlocalized value. We resolve the locale once at
//! parse time and keep only the winning value per key.

/// The locale chain to try, most-specific first.
///
/// Given `"zh_CN.UTF-8@pinyin"` this returns
/// `["zh_CN@pinyin", "zh_CN", "zh@pinyin", "zh"]`. The unlocalized (base)
/// value is always the implicit final fallback handled by the parser.
#[derive(Debug, Clone)]
pub struct Locale {
    variants: Vec<String>,
}

impl Locale {
    /// Build from a raw locale string such as `"en_US.UTF-8"` or `"zh_CN"`.
    /// Anything missing or empty collapses to the empty locale, which means
    /// "only the unlocalized value matches".
    pub fn parse(raw: &str) -> Locale {
        // Format: language[_territory][.codeset][@modifier]
        // (e.g. `zh_CN.UTF-8@pinyin`). The modifier trails the codeset, so
        // split it off the whole string first, then the codeset, then the
        // territory.
        let (lang_codeset, modifier) = match raw.split_once('@') {
            Some((lc, m)) => (lc, Some(m)),
            None => (raw, None),
        };
        let lang_country = lang_codeset.split('.').next().unwrap_or("");
        let (lang, country) = match lang_country.split_once('_') {
            Some((l, c)) => (l, Some(c)),
            None => (lang_country, None),
        };

        let mut variants = Vec::with_capacity(4);
        if let Some(c) = country {
            if let Some(m) = modifier {
                variants.push(format!("{lang}_{c}@{m}"));
            }
            variants.push(format!("{lang}_{c}"));
        }
        if let Some(m) = modifier {
            variants.push(format!("{lang}@{m}"));
        }
        if !lang.is_empty() {
            variants.push(lang.to_string());
        }
        Locale { variants }
    }

    /// Iterate the locale variants, most-specific first.
    pub fn variants(&self) -> &[String] {
        &self.variants
    }
}

/// Determine the current host locale from the standard environment
/// precedence: `LC_ALL` > `LC_MESSAGES` > `LANG`. Returns an empty locale
/// when none are set (so only unlocalized values match).
pub fn current_locale() -> Locale {
    for var in ["LC_ALL", "LC_MESSAGES", "LANG"] {
        if let Some(v) = std::env::var(var).ok().filter(|s| !s.is_empty()) {
            return Locale::parse(&v);
        }
    }
    Locale::parse("")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_locale_chain_with_modifier_and_codeset() {
        let l = Locale::parse("zh_CN.UTF-8@pinyin");
        assert_eq!(
            l.variants(),
            &["zh_CN@pinyin", "zh_CN", "zh@pinyin", "zh"].map(String::from)
        );
    }

    #[test]
    fn lang_country_without_modifier() {
        let l = Locale::parse("en_US.UTF-8");
        assert_eq!(l.variants(), &["en_US".to_string(), "en".to_string()]);
    }

    #[test]
    fn bare_lang() {
        let l = Locale::parse("de");
        assert_eq!(l.variants(), &["de".to_string()]);
    }

    #[test]
    fn empty_locale_matches_nothing_specific() {
        assert!(Locale::parse("").variants().is_empty());
    }
}
