use std::fmt;

macro_rules! integer_id {
    ($(#[$meta:meta])* $name:ident, $inner:ty) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
        pub struct $name($inner);

        impl $name {
            pub const fn new(value: $inner) -> Self {
                Self(value)
            }

            pub const fn get(self) -> $inner {
                self.0
            }
        }

        impl From<$inner> for $name {
            fn from(value: $inner) -> Self {
                Self::new(value)
            }
        }

        impl From<$name> for $inner {
            fn from(value: $name) -> Self {
                value.get()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(formatter)
            }
        }
    };
}

integer_id!(
    /// Stable local identifier for a user-visible logical file.
    LogicalFileId,
    u64
);
integer_id!(
    /// Stable local identifier for a Telegram-backed remote object.
    RemoteObjectId,
    u64
);
integer_id!(
    /// Stable local identifier for a multipart package.
    PackageId,
    u64
);
integer_id!(
    /// Stable local identifier for a logical-file transfer.
    TransferId,
    u64
);
integer_id!(
    /// Stable local identifier for a collection.
    CollectionId,
    u64
);
integer_id!(
    /// Stable local identifier for an indexing job.
    IndexJobId,
    u64
);
integer_id!(
    /// Stable local Telegram account identifier.
    AccountId,
    i64
);
integer_id!(
    /// Telegram chat/channel identity scoped by an [`AccountId`](crate::AccountId).
    ChatId,
    i64
);
integer_id!(
    /// Telegram message identifier scoped by account and chat.
    MessageId,
    i64
);
integer_id!(
    /// Zero-based application-part index within a package or transfer.
    PartIndex,
    u32
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unrelated_identifiers_remain_distinct_types() {
        let logical_file_id = LogicalFileId::new(7);
        let transfer_id = TransferId::new(7);

        assert_eq!(logical_file_id.get(), transfer_id.get());
        assert_eq!(logical_file_id.to_string(), "7");
    }
}
