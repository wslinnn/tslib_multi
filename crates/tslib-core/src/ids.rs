//! Typed identifiers for the higher-level convenience APIs.
//!
//! The existing methods keep plain `u16`/`u64` for source compatibility. New
//! command/view APIs use these wrappers so a client id cannot be passed where a
//! channel id is expected.

macro_rules! id_type {
    ($name:ident, $inner:ty, $doc:literal) => {
        #[doc = $doc]
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
        pub struct $name(pub $inner);

        impl From<$inner> for $name {
            fn from(value: $inner) -> Self {
                Self(value)
            }
        }

        impl From<$name> for $inner {
            fn from(value: $name) -> Self {
                value.0
            }
        }
    };
}

id_type!(ClientId, u16, "Session client id.");
id_type!(ChannelId, u64, "Channel id.");
id_type!(DatabaseId, u64, "Persistent database id.");
id_type!(ServerGroupId, u64, "Server group id.");
id_type!(ChannelGroupId, u64, "Channel group id.");
id_type!(PermissionId, u32, "Permission id.");
id_type!(IconId, i64, "Icon id.");
