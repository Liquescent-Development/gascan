#![forbid(unsafe_code)]

//! Generated Gas Can local control-plane API, version 1.

/// Supported API major version.
pub const API_MAJOR: u32 = 1;
/// Current backwards-compatible API minor version.
pub const API_MINOR: u32 = 0;

/// Stable machine-readable values carried by [`v1::Error::code`].
pub mod error_code {
    /// The peer requested a different API major version.
    pub const INCOMPATIBLE_API_MAJOR: &str = "incompatible_api_major";
    /// A request failed validation.
    pub const INVALID_REQUEST: &str = "invalid_request";
    /// The selected sandbox does not exist.
    pub const SANDBOX_NOT_FOUND: &str = "sandbox_not_found";
    /// Another operation prevents this request from running.
    pub const OPERATION_CONFLICT: &str = "operation_conflict";
    /// The selected runtime cannot perform the request.
    pub const BACKEND_UNAVAILABLE: &str = "backend_unavailable";
    /// The daemon encountered an unexpected internal failure.
    pub const INTERNAL: &str = "internal";

    /// All codes defined by API v1.
    pub const ALL: &[&str] = &[
        INCOMPATIBLE_API_MAJOR,
        INVALID_REQUEST,
        SANDBOX_NOT_FOUND,
        OPERATION_CONFLICT,
        BACKEND_UNAVAILABLE,
        INTERNAL,
    ];
}

/// A validated operation identifier suitable for the durable signed store.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct CheckedOperationId(i64);

impl CheckedOperationId {
    /// Returns the validated positive identifier.
    #[must_use]
    pub const fn get(self) -> i64 {
        self.0
    }
}

/// Reason a wire operation identifier cannot be used by the durable store.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OperationIdError {
    /// Zero is reserved to mean that no operation is present.
    Zero,
    /// The unsigned wire value cannot fit in the signed durable representation.
    OutOfRange,
}

impl TryFrom<u64> for CheckedOperationId {
    type Error = OperationIdError;

    fn try_from(value: u64) -> Result<Self, Self::Error> {
        if value == 0 {
            return Err(OperationIdError::Zero);
        }
        i64::try_from(value)
            .map(Self)
            .map_err(|_| OperationIdError::OutOfRange)
    }
}

impl From<CheckedOperationId> for v1::OperationId {
    fn from(value: CheckedOperationId) -> Self {
        Self {
            value: value.get() as u64,
        }
    }
}

impl TryFrom<v1::OperationId> for CheckedOperationId {
    type Error = OperationIdError;

    fn try_from(value: v1::OperationId) -> Result<Self, Self::Error> {
        Self::try_from(value.value)
    }
}

/// Encoded protobuf descriptors for compatibility checks and reflection.
pub const FILE_DESCRIPTOR_SET: &[u8] = tonic::include_file_descriptor_set!("gascan_descriptor");

/// Generated version 1 messages and Tonic client/server traits.
pub mod v1 {
    tonic::include_proto!("gascan.v1");
}

/// A handshake failure detected before serving an API request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HandshakeRejection {
    /// The peer requested an incompatible API major version.
    IncompatibleApiMajor { supported: u32, requested: u32 },
}

impl HandshakeRejection {
    /// Stable machine-readable error code used on the wire.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::IncompatibleApiMajor { .. } => error_code::INCOMPATIBLE_API_MAJOR,
        }
    }
}

/// Checks whether a peer can use this API implementation.
pub const fn validate_api_major(requested: u32) -> Result<(), HandshakeRejection> {
    if requested == API_MAJOR {
        Ok(())
    } else {
        Err(HandshakeRejection::IncompatibleApiMajor {
            supported: API_MAJOR,
            requested,
        })
    }
}
