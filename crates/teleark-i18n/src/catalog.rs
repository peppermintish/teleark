use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;

use fluent_bundle::FluentResource;
use fluent_bundle::concurrent::FluentBundle as ConcurrentFluentBundle;

use crate::{MessageArgs, MessageId, SupportedLocale};

#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ResourceValidationError {
    Parse {
        locale: SupportedLocale,
        details: String,
    },
    DuplicateMessage {
        locale: SupportedLocale,
        message_id: String,
    },
    InvalidMessageId {
        locale: SupportedLocale,
        message_id: String,
    },
    KeyMismatch {
        locale: SupportedLocale,
        missing: Vec<String>,
        unexpected: Vec<String>,
    },
    VariableMismatch {
        locale: SupportedLocale,
        message_id: String,
        expected: Vec<String>,
        actual: Vec<String>,
    },
}

impl fmt::Display for ResourceValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Parse { locale, details } => {
                write!(
                    formatter,
                    "{} Fluent resource is invalid: {details}",
                    locale.as_str()
                )
            }
            Self::DuplicateMessage { locale, message_id } => write!(
                formatter,
                "{} resource contains duplicate message {message_id}",
                locale.as_str()
            ),
            Self::InvalidMessageId { locale, message_id } => write!(
                formatter,
                "{} resource contains invalid message ID {message_id}",
                locale.as_str()
            ),
            Self::KeyMismatch {
                locale,
                missing,
                unexpected,
            } => write!(
                formatter,
                "{} keys differ from en-US; missing: {missing:?}; unexpected: {unexpected:?}",
                locale.as_str()
            ),
            Self::VariableMismatch {
                locale,
                message_id,
                expected,
                actual,
            } => write!(
                formatter,
                "{} message {message_id} variables differ; expected {expected:?}, found {actual:?}",
                locale.as_str()
            ),
        }
    }
}

impl Error for ResourceValidationError {}

#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum LocalizationError {
    MissingMessage {
        message_id: MessageId,
    },
    MissingValue {
        message_id: MessageId,
    },
    Formatting {
        locale: SupportedLocale,
        message_id: MessageId,
        details: String,
    },
}

impl fmt::Display for LocalizationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingMessage { message_id } => {
                write!(formatter, "localization message {message_id} is missing")
            }
            Self::MissingValue { message_id } => {
                write!(formatter, "localization message {message_id} has no value")
            }
            Self::Formatting {
                locale,
                message_id,
                details,
            } => write!(
                formatter,
                "could not format {} message {message_id}: {details}",
                locale.as_str()
            ),
        }
    }
}

impl Error for LocalizationError {}

struct Catalog {
    bundle: ConcurrentFluentBundle<FluentResource>,
    variables: BTreeMap<String, BTreeSet<String>>,
}

impl fmt::Debug for Catalog {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Catalog")
            .field("variables", &self.variables)
            .finish_non_exhaustive()
    }
}

impl Catalog {
    fn parse(locale: SupportedLocale, source: &str) -> Result<Self, ResourceValidationError> {
        let variables = parse_metadata(locale, source)?;
        let resource = FluentResource::try_new(source.to_owned()).map_err(|(_, errors)| {
            ResourceValidationError::Parse {
                locale,
                details: format!("{errors:?}"),
            }
        })?;
        let mut bundle = ConcurrentFluentBundle::new_concurrent(vec![locale.language_identifier()]);
        // All registered locales are left-to-right, and UI tests/copy actions
        // require translations without invisible FSI/PDI marks. Revisit this when
        // adding a right-to-left locale or interpolating untrusted user content.
        bundle.set_use_isolating(false);
        bundle
            .add_resource(resource)
            .map_err(|errors| ResourceValidationError::Parse {
                locale,
                details: format!("{errors:?}"),
            })?;
        Ok(Self { bundle, variables })
    }
}

/// Validates syntax, duplicates, known keys and variable parity for all catalogs.
/// Complete catalogs additionally require every canonical key. Incremental
/// catalogs inherit missing messages through the validated English fallback.
pub fn validate_embedded_resources() -> Result<(), ResourceValidationError> {
    let catalogs = parse_embedded_catalogs()?;
    validate_catalog_parity(&catalogs)
}

fn parse_embedded_catalogs() -> Result<BTreeMap<SupportedLocale, Catalog>, ResourceValidationError>
{
    SupportedLocale::ALL
        .into_iter()
        .map(|locale| {
            Catalog::parse(locale, locale.embedded_source()).map(|catalog| (locale, catalog))
        })
        .collect()
}

fn validate_catalog_parity(
    catalogs: &BTreeMap<SupportedLocale, Catalog>,
) -> Result<(), ResourceValidationError> {
    let canonical = &catalogs[&SupportedLocale::FALLBACK].variables;
    let canonical_keys: BTreeSet<&String> = canonical.keys().collect();

    for locale in SupportedLocale::ALL {
        let catalog = &catalogs[&locale];
        let locale_keys: BTreeSet<&String> = catalog.variables.keys().collect();
        let missing: Vec<String> = canonical_keys
            .difference(&locale_keys)
            .map(|key| (*key).clone())
            .collect();
        let unexpected: Vec<String> = locale_keys
            .difference(&canonical_keys)
            .map(|key| (*key).clone())
            .collect();
        if (locale.has_complete_catalog() && !missing.is_empty()) || !unexpected.is_empty() {
            return Err(ResourceValidationError::KeyMismatch {
                locale,
                missing,
                unexpected,
            });
        }

        for (message_id, expected_variables) in canonical {
            let Some(actual_variables) = catalog.variables.get(message_id) else {
                // An untranslated message is resolved through the complete
                // English catalog; never clone English into a translated file.
                continue;
            };
            if actual_variables != expected_variables {
                return Err(ResourceValidationError::VariableMismatch {
                    locale,
                    message_id: message_id.clone(),
                    expected: expected_variables.iter().cloned().collect(),
                    actual: actual_variables.iter().cloned().collect(),
                });
            }
        }
    }
    Ok(())
}

fn parse_metadata(
    locale: SupportedLocale,
    source: &str,
) -> Result<BTreeMap<String, BTreeSet<String>>, ResourceValidationError> {
    let mut messages = BTreeMap::<String, BTreeSet<String>>::new();
    let mut current_id: Option<String> = None;

    for line in source.lines() {
        if !line.starts_with(char::is_whitespace)
            && !line.is_empty()
            && !line.starts_with('#')
            && let Some((candidate, value)) = line.split_once('=')
        {
            let message_id = candidate.trim();
            if !valid_fluent_message_id(message_id) {
                return Err(ResourceValidationError::InvalidMessageId {
                    locale,
                    message_id: message_id.to_owned(),
                });
            }
            if messages.contains_key(message_id) {
                return Err(ResourceValidationError::DuplicateMessage {
                    locale,
                    message_id: message_id.to_owned(),
                });
            }
            messages.insert(message_id.to_owned(), extract_variables(value));
            current_id = Some(message_id.to_owned());
            continue;
        }

        if let Some(message_id) = &current_id {
            let Some(variables) = messages.get_mut(message_id) else {
                return Err(ResourceValidationError::Parse {
                    locale,
                    details: "resource metadata parser lost its current message".to_owned(),
                });
            };
            variables.extend(extract_variables(line));
        }
    }
    Ok(messages)
}

fn valid_fluent_message_id(message_id: &str) -> bool {
    let mut chars = message_id.chars();
    chars
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic())
        && chars.all(|character| {
            character.is_ascii_alphanumeric() || character == '-' || character == '_'
        })
}

fn extract_variables(text: &str) -> BTreeSet<String> {
    let mut variables = BTreeSet::new();
    let mut characters = text.char_indices().peekable();
    while let Some((_, character)) = characters.next() {
        if character != '$' {
            continue;
        }
        let mut name = String::new();
        while let Some((_, next)) = characters.peek() {
            if next.is_ascii_alphanumeric() || matches!(next, '_' | '-') {
                name.push(*next);
                characters.next();
            } else {
                break;
            }
        }
        if !name.is_empty() {
            variables.insert(name);
        }
    }
    variables
}

/// Runtime-selectable localization service reusable by GUI and CLI frontends.
pub struct Localizer {
    selected: SupportedLocale,
    catalogs: BTreeMap<SupportedLocale, Catalog>,
}

impl Localizer {
    pub fn new(selected: SupportedLocale) -> Result<Self, ResourceValidationError> {
        let catalogs = parse_embedded_catalogs()?;
        validate_catalog_parity(&catalogs)?;
        Ok(Self { selected, catalogs })
    }

    pub const fn locale(&self) -> SupportedLocale {
        self.selected
    }

    /// Switches language immediately; no persistence or application restart is required.
    pub fn set_locale(&mut self, locale: SupportedLocale) {
        self.selected = locale;
    }

    pub fn translate(&self, message_id: MessageId) -> Result<String, LocalizationError> {
        self.translate_with(message_id, &MessageArgs::new())
    }

    pub fn translate_with(
        &self,
        message_id: MessageId,
        args: &MessageArgs,
    ) -> Result<String, LocalizationError> {
        let fluent_key = message_id.fluent_key();
        if self.catalogs[&self.selected]
            .bundle
            .has_message(fluent_key.as_ref())
        {
            return self.format_from(self.selected, message_id, args);
        }
        if self.selected != SupportedLocale::FALLBACK
            && self.catalogs[&SupportedLocale::FALLBACK]
                .bundle
                .has_message(fluent_key.as_ref())
        {
            return self.format_from(SupportedLocale::FALLBACK, message_id, args);
        }
        Err(LocalizationError::MissingMessage { message_id })
    }

    pub fn translate_or_id(&self, message_id: MessageId) -> String {
        self.translate(message_id)
            .unwrap_or_else(|_| message_id.to_string())
    }

    pub fn translate_with_or_id(&self, message_id: MessageId, args: &MessageArgs) -> String {
        self.translate_with(message_id, args)
            .unwrap_or_else(|_| message_id.to_string())
    }

    /// Whether this message is available in the locale or its English fallback.
    pub fn contains(&self, locale: SupportedLocale, message_id: MessageId) -> bool {
        let fluent_key = message_id.fluent_key();
        self.catalogs[&locale]
            .bundle
            .has_message(fluent_key.as_ref())
            || self.catalogs[&SupportedLocale::FALLBACK]
                .bundle
                .has_message(fluent_key.as_ref())
    }

    fn format_from(
        &self,
        locale: SupportedLocale,
        message_id: MessageId,
        args: &MessageArgs,
    ) -> Result<String, LocalizationError> {
        let bundle = &self.catalogs[&locale].bundle;
        let fluent_key = message_id.fluent_key();
        let message = bundle
            .get_message(fluent_key.as_ref())
            .ok_or(LocalizationError::MissingMessage { message_id })?;
        let pattern = message
            .value()
            .ok_or(LocalizationError::MissingValue { message_id })?;
        let fluent_args = args.to_fluent_args();
        let mut errors = Vec::new();
        let value = bundle.format_pattern(
            pattern,
            (!args.is_empty()).then_some(&fluent_args),
            &mut errors,
        );
        if errors.is_empty() {
            Ok(value.into_owned())
        } else {
            Err(LocalizationError::Formatting {
                locale,
                message_id,
                details: format!("{errors:?}"),
            })
        }
    }

    #[cfg(test)]
    fn from_sources_unchecked(
        selected: SupportedLocale,
        sources: &[(SupportedLocale, &str)],
    ) -> Result<Self, ResourceValidationError> {
        let catalogs = sources
            .iter()
            .map(|(locale, source)| {
                Catalog::parse(*locale, source).map(|catalog| (*locale, catalog))
            })
            .collect::<Result<_, _>>()?;
        Ok(Self { selected, catalogs })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message_ids;
    use fluent_bundle::FluentValue;

    #[test]
    fn embedded_resources_parse_and_have_key_and_variable_parity() {
        validate_embedded_resources().unwrap();
    }

    #[test]
    fn expanded_catalogs_cover_primary_flows_and_resolve_all_translated_messages() {
        let catalogs = parse_embedded_catalogs().unwrap();
        let required = [
            "nav-settings",
            "nav-transfers",
            "common-cancel",
            "common-save",
            "telegram-login-title",
            "telegram-batch-download-action",
            "upload-dialog-title",
            "settings-language-title",
            "settings-keychain-disable-warning",
            "channel-filter-batch-help",
            "channel-filter-batch-phase-complete",
            "settings-language-fallback-note",
            "file-detail-title",
            "transfer-state-failed",
        ];
        for locale in SupportedLocale::ALL {
            let catalog = &catalogs[&locale];
            for id in required {
                assert!(
                    catalog.bundle.has_message(id),
                    "{} lacks {id}",
                    locale.as_str()
                );
            }
            if !locale.has_complete_catalog() {
                assert!(
                    catalog.variables.len() >= 650,
                    "{} translation coverage regressed",
                    locale.as_str()
                );
            }
            // Exercise real Fluent references, variables and plural selection,
            // without opening or rendering any non-English interface.
            for (id, variables) in &catalog.variables {
                let mut args = fluent_bundle::FluentArgs::new();
                for variable in variables {
                    args.set(variable.as_str(), 2);
                }
                let pattern = catalog.bundle.get_message(id).unwrap().value().unwrap();
                let mut errors = Vec::new();
                let output = catalog
                    .bundle
                    .format_pattern(pattern, Some(&args), &mut errors);
                assert!(errors.is_empty(), "{} {id}: {errors:?}", locale.as_str());
                assert!(!output.is_empty(), "{} {id} is empty", locale.as_str());
            }
        }
    }

    #[test]
    fn incremental_catalogs_reject_unknown_keys_and_incompatible_variables() {
        let mut catalogs = parse_embedded_catalogs().unwrap();
        let locale = SupportedLocale::EsEs;
        catalogs.insert(
            locale,
            Catalog::parse(locale, "invented-message = desconocido").unwrap(),
        );
        assert!(matches!(
            validate_catalog_parity(&catalogs),
            Err(ResourceValidationError::KeyMismatch { .. })
        ));
        catalogs.insert(
            locale,
            Catalog::parse(locale, "common-save = Guardar { $unexpected }").unwrap(),
        );
        assert!(matches!(
            validate_catalog_parity(&catalogs),
            Err(ResourceValidationError::VariableMismatch { .. })
        ));
    }

    #[test]
    fn every_english_message_formats_for_each_registered_language() {
        let english = Localizer::new(SupportedLocale::EnUs).unwrap();
        let mut args = MessageArgs::new();
        for tail in SupportedLocale::FALLBACK
            .embedded_source()
            .split('$')
            .skip(1)
        {
            let length = tail
                .find(|character: char| {
                    !character.is_ascii_alphanumeric() && character != '_' && character != '-'
                })
                .unwrap_or(tail.len());
            args = args.with(&tail[..length], 2_u64);
        }
        let ids: Vec<_> = SupportedLocale::FALLBACK
            .embedded_source()
            .lines()
            .filter(|line| !line.starts_with(char::is_whitespace) && !line.starts_with('#'))
            .filter_map(|line| {
                line.split_once('=')
                    .map(|(key, _)| MessageId::new(key.trim()))
            })
            .collect();
        for locale in SupportedLocale::ALL {
            let selected = Localizer::new(locale).unwrap();
            for id in &ids {
                assert!(selected.contains(locale, *id));
                assert!(
                    selected.translate_with(*id, &args).is_ok(),
                    "{} {id}",
                    locale.as_str()
                );
            }
            if !locale.has_complete_catalog() {
                let fallback_id = MessageId::new("about-changelog-v040");
                assert_eq!(
                    selected.translate(fallback_id).unwrap(),
                    english.translate(fallback_id).unwrap()
                );
            }
        }
    }

    #[test]
    fn runtime_locale_switching_changes_message_without_rebuild() {
        let mut localizer = Localizer::new(SupportedLocale::EnUs).unwrap();
        assert_eq!(
            localizer.translate(message_ids::library::TITLE).unwrap(),
            "Library"
        );

        localizer.set_locale(SupportedLocale::JaJp);
        assert_eq!(
            localizer.translate(message_ids::library::TITLE).unwrap(),
            "ライブラリ"
        );
    }

    #[test]
    fn fluent_plural_and_variables_are_resolved() {
        let localizer = Localizer::new(SupportedLocale::EnUs).unwrap();
        let one = MessageArgs::new().with("count", 1_u64);
        let many = MessageArgs::new().with("count", 2_u64);

        assert_eq!(
            localizer
                .translate_with(message_ids::library::ITEM_COUNT, &one)
                .unwrap(),
            "1 item"
        );
        assert_eq!(
            localizer
                .translate_with(message_ids::library::ITEM_COUNT, &many)
                .unwrap(),
            "2 items"
        );
    }

    #[test]
    fn missing_selected_message_uses_english_fallback() {
        let localizer = Localizer::from_sources_unchecked(
            SupportedLocale::ZhCn,
            &[
                (SupportedLocale::EnUs, "fallback-only = English fallback"),
                (SupportedLocale::ZhCn, "localized-only = 本地消息"),
            ],
        )
        .unwrap();

        assert_eq!(
            localizer
                .translate(MessageId::new("fallback-only"))
                .unwrap(),
            "English fallback"
        );
    }

    #[test]
    fn missing_all_catalogs_returns_stable_id() {
        let localizer = Localizer::new(SupportedLocale::ZhCn).unwrap();
        let missing = MessageId::new("does-not-exist");
        assert_eq!(
            localizer.translate(missing),
            Err(LocalizationError::MissingMessage {
                message_id: missing
            })
        );
        assert_eq!(localizer.translate_or_id(missing), "does-not-exist");
    }

    #[test]
    fn legacy_dotted_domain_ids_are_canonicalized_for_fluent_lookup() {
        let localizer = Localizer::new(SupportedLocale::EnUs).unwrap();
        assert_eq!(
            localizer
                .translate(MessageId::new("file.type.disk_image"))
                .unwrap(),
            "Disk Image"
        );
        assert_eq!(
            localizer
                .translate(MessageId::new("transfer.state.uploading"))
                .unwrap(),
            "Uploading"
        );
    }

    #[test]
    fn parser_reports_duplicate_ids_before_runtime_bundle_creation() {
        let error = Catalog::parse(
            SupportedLocale::EnUs,
            "duplicate = First\nduplicate = Second",
        )
        .unwrap_err();
        assert_eq!(
            error,
            ResourceValidationError::DuplicateMessage {
                locale: SupportedLocale::EnUs,
                message_id: "duplicate".to_owned(),
            }
        );
    }

    #[test]
    fn fluent_values_support_owned_text_without_leaking_lifetimes() {
        let args = MessageArgs::new().with("value", String::from("example"));
        let fluent = args.to_fluent_args();
        assert!(matches!(fluent.get("value"), Some(FluentValue::String(_))));
    }

    #[test]
    fn localizer_can_cross_frontend_worker_boundaries() {
        fn assert_send_and_sync<T: Send + Sync>() {}
        assert_send_and_sync::<Localizer>();
    }
}
