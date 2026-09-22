use unic_langid::{LanguageIdentifier, langid};

// Register each language once. The catalog, settings selector and locale parser
// all consume this registry; no frontend should maintain its own locale list.
macro_rules! supported_locales {
    ($($variant:ident => ($tag:literal, $name:literal, $badge:literal, $complete:literal)),+ $(,)?) => {
        #[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
        pub enum SupportedLocale {
            #[default]
            EnUs,
            $($variant,)+
        }

        impl SupportedLocale {
            pub const ALL: [Self; 1 + supported_locales!(@count $($variant),+)] =
                [Self::EnUs, $(Self::$variant,)+];
            pub const FALLBACK: Self = Self::EnUs;

            pub const fn as_str(self) -> &'static str {
                match self { Self::EnUs => "en-US", $(Self::$variant => $tag,)+ }
            }

            /// Native language name, readable before localization loads.
            pub const fn autonym(self) -> &'static str {
                match self { Self::EnUs => "English", $(Self::$variant => $name,)+ }
            }

            pub const fn badge(self) -> &'static str {
                match self { Self::EnUs => "A", $(Self::$variant => $badge,)+ }
            }

            pub fn language_identifier(self) -> LanguageIdentifier {
                match self { Self::EnUs => langid!("en-US"), $(Self::$variant => langid!($tag),)+ }
            }

            pub(crate) const fn embedded_source(self) -> &'static str {
                match self {
                    Self::EnUs => include_str!("../resources/en-US/main.ftl"),
                    $(Self::$variant => include_str!(concat!("../resources/", $tag, "/main.ftl")),)+
                }
            }

            /// Complete catalogs require exact source-key parity. Incremental
            /// catalogs must match every translated key and use English for gaps.
            pub const fn has_complete_catalog(self) -> bool {
                match self { Self::EnUs => true, $(Self::$variant => $complete,)+ }
            }
        }
    };
    (@count $head:ident $(,$tail:ident)*) => {1 $(+ supported_locales!(@count $tail))*};
}

supported_locales! {
    ZhCn => ("zh-CN", "简体中文", "中", true),
    JaJp => ("ja-JP", "日本語", "あ", true),
    EsEs => ("es-ES", "Español", "ES", false),
    FrFr => ("fr-FR", "Français", "FR", false),
    DeDe => ("de-DE", "Deutsch", "DE", false),
    PtBr => ("pt-BR", "Português (Brasil)", "PT", false),
    RuRu => ("ru-RU", "Русский", "РУ", false),
    KoKr => ("ko-KR", "한국어", "한", false),
    HiIn => ("hi-IN", "हिन्दी", "हि", false),
}

impl SupportedLocale {
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
            .find_map(|locale| Self::parse_supported(locale.as_ref()))
            .unwrap_or(Self::FALLBACK)
    }

    /// Parses a supported language without substituting the English fallback.
    pub fn parse_supported(requested: &str) -> Option<Self> {
        let without_encoding = requested
            .split_once('.')
            .map_or(requested, |(locale, _)| locale);
        let without_modifier = without_encoding
            .split_once('@')
            .map_or(without_encoding, |(locale, _)| locale);
        let normalized = without_modifier.replace('_', "-");
        let identifier: LanguageIdentifier = normalized.parse().ok()?;
        if let Some(locale) = Self::ALL
            .into_iter()
            .find(|locale| locale.language_identifier() == identifier)
        {
            return Some(locale);
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
            _ => Self::ALL
                .into_iter()
                .find(|locale| locale.language_identifier().language == identifier.language),
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
        assert_eq!(SupportedLocale::negotiate(["fr-FR"]), SupportedLocale::FrFr);
        assert_eq!(SupportedLocale::negotiate(["ar-SA"]), SupportedLocale::EnUs);
    }

    #[test]
    fn registered_locales_round_trip_and_accept_os_spellings() {
        for locale in SupportedLocale::ALL {
            assert_eq!(
                SupportedLocale::parse_supported(locale.as_str()),
                Some(locale)
            );
            assert_eq!(
                SupportedLocale::parse_supported(&format!(
                    "{}.UTF-8",
                    locale.as_str().replace('-', "_")
                )),
                Some(locale)
            );
            assert!(!locale.autonym().is_empty());
            assert!(!locale.badge().is_empty());
        }
        assert_eq!(
            SupportedLocale::parse_supported("es-MX"),
            Some(SupportedLocale::EsEs)
        );
        assert_eq!(
            SupportedLocale::parse_supported("pt-PT"),
            Some(SupportedLocale::PtBr)
        );
        assert_eq!(
            SupportedLocale::parse_supported("fr-CA"),
            Some(SupportedLocale::FrFr)
        );
        assert_eq!(SupportedLocale::parse_supported("not_a_locale"), None);
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
