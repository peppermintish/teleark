//! Frontend-independent internationalization for TeleArk.
//!
//! Message selection and plural rules use Project Fluent. Number and binary
//! byte-unit formatting is intentionally centralized here so frontends do not
//! acquire subtly different policies.
//!
//! The TeleArk wrapper intentionally resolves top-level Fluent message values.
//! It does not expose Fluent terms or attributes yet; add explicit validation
//! and lookup APIs before introducing either into the catalogs. Variables,
//! selectors, and locale-aware Fluent plural rules are supported.

mod catalog;
mod error_mapping;
pub mod format;
mod locale;
mod message;

pub use catalog::{
    LocalizationError, Localizer, ResourceValidationError, validate_embedded_resources,
};
pub use error_mapping::MessageDescriptor;
pub use locale::{LocalePreference, SupportedLocale};
pub use message::{MessageArgs, MessageArgument, MessageId, message_ids};
