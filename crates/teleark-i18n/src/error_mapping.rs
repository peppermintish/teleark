use teleark_core::TransferError;

use crate::{LocalizationError, Localizer, MessageArgs, MessageId, message_ids};

/// A stable localization message plus structured interpolation arguments.
#[derive(Clone, Debug, PartialEq)]
pub struct MessageDescriptor {
    pub id: MessageId,
    pub args: MessageArgs,
}

impl MessageDescriptor {
    pub const fn new(id: MessageId) -> Self {
        Self {
            id,
            args: MessageArgs::new(),
        }
    }

    pub fn with_arg(
        mut self,
        name: &'static str,
        value: impl Into<crate::MessageArgument>,
    ) -> Self {
        self.args.set(name, value);
        self
    }
}

impl From<&TransferError> for MessageDescriptor {
    fn from(error: &TransferError) -> Self {
        use message_ids::error as ids;

        match error {
            TransferError::Network => Self::new(ids::TRANSFER_NETWORK),
            TransferError::FloodWait { retry_after } => {
                Self::new(ids::TRANSFER_FLOOD_WAIT).with_arg("seconds", retry_after.as_secs())
            }
            TransferError::Authorization => Self::new(ids::TRANSFER_AUTHORIZATION),
            TransferError::SourceMissing => Self::new(ids::TRANSFER_SOURCE_MISSING),
            TransferError::SourceChanged => Self::new(ids::TRANSFER_SOURCE_CHANGED),
            TransferError::DiskFull => Self::new(ids::TRANSFER_DISK_FULL),
            TransferError::PermissionDenied => Self::new(ids::TRANSFER_PERMISSION_DENIED),
            TransferError::RemoteMissing => Self::new(ids::TRANSFER_REMOTE_MISSING),
            TransferError::HashMismatch => Self::new(ids::TRANSFER_HASH_MISMATCH),
            TransferError::AuthenticationFailed => Self::new(ids::TRANSFER_AUTHENTICATION_FAILED),
            TransferError::ManifestCorrupted => Self::new(ids::TRANSFER_MANIFEST_CORRUPTED),
            TransferError::UnsupportedManifestVersion { version } => {
                Self::new(ids::TRANSFER_UNSUPPORTED_MANIFEST).with_arg("version", *version)
            }
            TransferError::KeyUnavailable => Self::new(ids::TRANSFER_KEY_UNAVAILABLE),
            TransferError::WrongPassword => Self::new(ids::TRANSFER_WRONG_PASSWORD),
            TransferError::Database => Self::new(ids::TRANSFER_DATABASE),
            TransferError::Cancelled => Self::new(ids::TRANSFER_CANCELLED),
            _ => Self::new(ids::TRANSFER_UNKNOWN),
        }
    }
}

impl Localizer {
    pub fn translate_descriptor(
        &self,
        descriptor: &MessageDescriptor,
    ) -> Result<String, LocalizationError> {
        self.translate_with(descriptor.id, &descriptor.args)
    }

    pub fn translate_transfer_error(
        &self,
        error: &TransferError,
    ) -> Result<String, LocalizationError> {
        self.translate_descriptor(&MessageDescriptor::from(error))
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::SupportedLocale;

    fn every_current_transfer_error() -> Vec<TransferError> {
        vec![
            TransferError::Network,
            TransferError::FloodWait {
                retry_after: Duration::from_secs(17),
            },
            TransferError::Authorization,
            TransferError::SourceMissing,
            TransferError::SourceChanged,
            TransferError::DiskFull,
            TransferError::PermissionDenied,
            TransferError::RemoteMissing,
            TransferError::HashMismatch,
            TransferError::AuthenticationFailed,
            TransferError::ManifestCorrupted,
            TransferError::UnsupportedManifestVersion { version: 9 },
            TransferError::KeyUnavailable,
            TransferError::WrongPassword,
            TransferError::Database,
            TransferError::Cancelled,
        ]
    }

    #[test]
    fn every_structured_transfer_error_maps_to_a_message_in_every_locale() {
        for locale in SupportedLocale::ALL {
            let localizer = Localizer::new(locale).unwrap();
            for error in every_current_transfer_error() {
                let descriptor = MessageDescriptor::from(&error);
                assert!(localizer.contains(locale, descriptor.id));
                assert!(
                    localizer.translate_descriptor(&descriptor).is_ok(),
                    "failed to translate {error:?} in {}",
                    locale.as_str()
                );
            }
        }
    }

    #[test]
    fn structured_error_parameters_are_preserved() {
        let descriptor = MessageDescriptor::from(&TransferError::FloodWait {
            retry_after: Duration::from_secs(17),
        });
        assert_eq!(descriptor.id, message_ids::error::TRANSFER_FLOOD_WAIT);
        assert_eq!(
            descriptor.args.get("seconds"),
            Some(&crate::MessageArgument::Number(17.0))
        );
    }
}
