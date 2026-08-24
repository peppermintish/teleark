use unic_langid::{LanguageIdentifier, langid};

#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum SupportedLocale {
    #[default]
    EnUs,
    ZhCn,
    JaJp,
}

impl SupportedLocale {
    pub const ALL: [Self; 3] = [Self::EnUs, Self::ZhCn, Self::JaJp];
    pub const FALLBACK: Self = Self::EnUs;

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::EnUs => "en-US",
            Self::ZhCn => "zh-CN",
            Self::JaJp => "ja-JP",
        }
    }

    /// Autonym suitable for the language selector even before localization loads.
    pub const fn autonym(self) -> &'static str {
        match self {
            Self::EnUs => "English",
            Self::ZhCn => "简体中文",
            Self::JaJp => "日本語",
        }
    }

    pub fn language_identifier(self) -> LanguageIdentifier {
        match self {
            Self::EnUs => langid!("en-US"),
            Self::ZhCn => langid!("zh-CN"),
            Self::JaJp => langid!("ja-JP"),
        }
    }

    /// Negotiates the first supported locale from OS-style preference strings.
    ///
    /// Regional English maps to `en-US`; Japanese maps to `ja-JP`; Simplified
    /// Chinese regions/scripts map to `zh-CN`. Traditional Chinese is not silently
    /// presented as Simplified Chinese and therefore falls through to the next
    /// preference or English.
    pub fn negotiate<I, S>(requested: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        requested
            .into_iter()
            .find_map(|locale| Self::match_requested(locale.as_ref()))
            .unwrap_or(Self::FALLBACK)
    }

    fn match_requested(requested: &str) -> Option<Self> {
        let without_encoding = requested
            .split_once('.')
            .map_or(requested, |(locale, _)| locale);
        let without_modifier = without_encoding
            .split_once('@')
            .map_or(without_encoding, |(locale, _)| locale);
        let normalized = without_modifier.replace('_', "-");
        let identifier: LanguageIdentifier = normalized.parse().ok()?;
        let canonical = identifier.to_string().to_ascii_lowercase();

        if canonical == "en-us" {
            return Some(Self::EnUs);
        }
        if canonical == "zh-cn" {
            return Some(Self::ZhCn);
        }
        if canonical == "ja-jp" {
            return Some(Self::JaJp);
        }

        let language = identifier.language.as_str();
        match language {
            "en" => Some(Self::EnUs),
            "ja" => Some(Self::JaJp),
            "zh" => {
                let traditional_script = identifier
                    .script
                    .as_ref()
                    .is_some_and(|script| script.as_str().eq_ignore_ascii_case("Hant"));
                let traditional_region = identifier
                    .region
                    .as_ref()
                    .is_some_and(|region| matches!(region.as_str(), "TW" | "HK" | "MO"));
                (!traditional_script && !traditional_region).then_some(Self::ZhCn)
            }
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum LocalePreference {
    #[default]
    SystemDefault,
    Explicit(SupportedLocale),
}

impl LocalePreference {
    /// Resolves a persisted preference against current operating-system locales.
    pub fn resolve<I, S>(self, system_locales: I) -> SupportedLocale
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        match self {
            Self::SystemDefault => SupportedLocale::negotiate(system_locales),
            Self::Explicit(locale) => locale,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locale_negotiation_matches_product_policy() {
        assert_eq!(SupportedLocale::negotiate(["en-AU"]), SupportedLocale::EnUs);
        assert_eq!(
            SupportedLocale::negotiate(["en_GB.UTF-8"]),
            SupportedLocale::EnUs
        );
        assert_eq!(SupportedLocale::negotiate(["zh-SG"]), SupportedLocale::ZhCn);
        assert_eq!(SupportedLocale::negotiate(["ja-JP"]), SupportedLocale::JaJp);
        assert_eq!(SupportedLocale::negotiate(["fr-FR"]), SupportedLocale::EnUs);
    }

    #[test]
    fn negotiation_uses_later_preference_before_fallback() {
        assert_eq!(
            SupportedLocale::negotiate(["zh-Hant-TW", "ja"]),
            SupportedLocale::JaJp
        );
        assert_eq!(SupportedLocale::negotiate(["zh-TW"]), SupportedLocale::EnUs);
    }

    #[test]
    fn explicit_user_preference_wins_over_system_locale() {
        assert_eq!(
            LocalePreference::Explicit(SupportedLocale::JaJp).resolve(["zh-CN"]),
            SupportedLocale::JaJp
        );
    }
}
