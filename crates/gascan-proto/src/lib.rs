#![forbid(unsafe_code)]

//! Generated Gas Can local control-plane API, version 1.

/// Supported API major version.
pub const API_MAJOR: u32 = 1;
/// Current backwards-compatible API minor version.
pub const API_MINOR: u32 = 0;
/// Required POSIX permission bits for the local socket directory (`0700`).
pub const SOCKET_DIRECTORY_MODE: u32 = 0o700;
/// Required POSIX permission bits for the local socket (`0600`).
pub const SOCKET_MODE: u32 = 0o600;
/// Stable code returned when an attach frame has no session token.
pub const SESSION_TOKEN_EMPTY: &str = "empty_session_token";

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
    /// An attach frame omitted its Run/Shell session token.
    pub const EMPTY_SESSION_TOKEN: &str = super::SESSION_TOKEN_EMPTY;
    /// A token does not identify a session known to this daemon.
    pub const UNKNOWN_SESSION_TOKEN: &str = "unknown_session_token";
    /// A known session token is no longer attachable.
    pub const EXPIRED_SESSION_TOKEN: &str = "expired_session_token";
    /// A later frame tried to change the session bound by the first frame.
    pub const SESSION_TOKEN_MISMATCH: &str = "session_token_mismatch";

    /// All codes defined by API v1.
    pub const ALL: &[&str] = &[
        INCOMPATIBLE_API_MAJOR,
        INVALID_REQUEST,
        SANDBOX_NOT_FOUND,
        OPERATION_CONFLICT,
        BACKEND_UNAVAILABLE,
        INTERNAL,
        EMPTY_SESSION_TOKEN,
        UNKNOWN_SESSION_TOKEN,
        EXPIRED_SESSION_TOKEN,
        SESSION_TOKEN_MISMATCH,
    ];
}

/// Reason an opaque Run/Shell attachment token is invalid.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SessionTokenError {
    /// Empty tokens cannot bind a frame to a session.
    Empty,
}

impl SessionTokenError {
    /// Stable machine-readable error code.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::Empty => SESSION_TOKEN_EMPTY,
        }
    }
}

/// Validates the opaque token carried by every attach frame.
pub const fn validate_session_token(token: &[u8]) -> Result<(), SessionTokenError> {
    if token.is_empty() {
        Err(SessionTokenError::Empty)
    } else {
        Ok(())
    }
}

/// Stateful validator that binds one Attach stream to exactly one session.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AttachSessionBinder {
    token: Option<Vec<u8>>,
}

impl AttachSessionBinder {
    /// Creates an unbound stream validator.
    #[must_use]
    pub const fn new() -> Self {
        Self { token: None }
    }

    /// Binds the first valid frame and requires the same token thereafter.
    pub fn validate_frame(&mut self, token: &[u8]) -> Result<(), AttachSessionError> {
        validate_session_token(token).map_err(|_| AttachSessionError::Empty)?;
        match &self.token {
            Some(bound) if bound.as_slice() != token => Err(AttachSessionError::Mismatch),
            Some(_) => Ok(()),
            None => {
                self.token = Some(token.to_vec());
                Ok(())
            }
        }
    }

    /// Returns the token established by the first valid frame, if any.
    #[must_use]
    pub fn session_token(&self) -> Option<&[u8]> {
        self.token.as_deref()
    }
}

/// A stream-local Attach session binding failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AttachSessionError {
    /// A frame omitted its session token.
    Empty,
    /// A later frame's token differs from the first frame's token.
    Mismatch,
}

impl AttachSessionError {
    /// Stable machine-readable error code.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::Empty => error_code::EMPTY_SESSION_TOKEN,
            Self::Mismatch => error_code::SESSION_TOKEN_MISMATCH,
        }
    }
}

/// Returns the only transport-security contract accepted by API v1.
#[must_use]
pub const fn local_transport_security() -> v1::TransportSecurity {
    v1::TransportSecurity {
        local_only: true,
        socket_directory_mode: SOCKET_DIRECTORY_MODE,
        socket_mode: SOCKET_MODE,
        require_same_user: true,
    }
}

/// Reason a reported local transport contract is unsafe or incompatible.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TransportSecurityError {
    /// The endpoint was not declared local-only.
    NotLocalOnly,
    /// Directory permission bits differ from `0700`.
    DirectoryMode,
    /// Socket permission bits differ from `0600`.
    SocketMode,
    /// Effective-UID authentication through Unix peer credentials is disabled.
    SameUserNotRequired,
}

/// Validates local-only transport, exact POSIX modes, and same-user authentication.
pub const fn validate_transport_security(
    security: &v1::TransportSecurity,
) -> Result<(), TransportSecurityError> {
    if !security.local_only {
        return Err(TransportSecurityError::NotLocalOnly);
    }
    if security.socket_directory_mode != SOCKET_DIRECTORY_MODE {
        return Err(TransportSecurityError::DirectoryMode);
    }
    if security.socket_mode != SOCKET_MODE {
        return Err(TransportSecurityError::SocketMode);
    }
    if !security.require_same_user {
        return Err(TransportSecurityError::SameUserNotRequired);
    }
    Ok(())
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

/// A validated positive sequence number in a durable operation event stream.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct CheckedEventSequence(i64);

impl CheckedEventSequence {
    /// Returns the validated positive sequence number.
    #[must_use]
    pub const fn get(self) -> i64 {
        self.0
    }
}

/// Reason a durable event sequence number is invalid.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EventSequenceError {
    /// Zero cannot identify an append-only event position.
    Zero,
    /// The unsigned wire value cannot fit in the signed durable representation.
    OutOfRange,
}

impl TryFrom<u64> for CheckedEventSequence {
    type Error = EventSequenceError;

    fn try_from(value: u64) -> Result<Self, Self::Error> {
        if value == 0 {
            return Err(EventSequenceError::Zero);
        }
        i64::try_from(value)
            .map(Self)
            .map_err(|_| EventSequenceError::OutOfRange)
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
