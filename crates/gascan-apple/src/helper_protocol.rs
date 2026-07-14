use base64::{Engine, engine::general_purpose::STANDARD};
use gascan_core::runtime::RuntimeError;
use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error as _};

use crate::{AttachInput, AttachOutput};

pub const HELPER_PROTOCOL_VERSION: u32 = 1;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HelperInput {
    Start {
        version: u32,
        container: String,
        argv: Vec<String>,
        tty: bool,
    },
    Stdin {
        version: u32,
        #[serde(with = "base64_bytes")]
        data: Vec<u8>,
    },
    Resize {
        version: u32,
        rows: u16,
        cols: u16,
    },
    Signal {
        version: u32,
        signal: i32,
    },
    Close {
        version: u32,
    },
}

impl HelperInput {
    pub fn start(container: String, argv: Vec<String>, tty: bool) -> Self {
        Self::Start {
            version: HELPER_PROTOCOL_VERSION,
            container,
            argv,
            tty,
        }
    }
}

impl From<AttachInput> for HelperInput {
    fn from(value: AttachInput) -> Self {
        match value {
            AttachInput::Stdin(data) => Self::Stdin {
                version: HELPER_PROTOCOL_VERSION,
                data,
            },
            AttachInput::Resize { rows, cols } => Self::Resize {
                version: HELPER_PROTOCOL_VERSION,
                rows,
                cols,
            },
            AttachInput::Signal(signal) => Self::Signal {
                version: HELPER_PROTOCOL_VERSION,
                signal,
            },
            AttachInput::Close => Self::Close {
                version: HELPER_PROTOCOL_VERSION,
            },
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HelperOutput {
    Stdout {
        version: u32,
        #[serde(with = "base64_bytes")]
        data: Vec<u8>,
    },
    Stderr {
        version: u32,
        #[serde(with = "base64_bytes")]
        data: Vec<u8>,
    },
    Error {
        version: u32,
        code: String,
        message: String,
    },
    Exit {
        version: u32,
        code: i32,
    },
}

impl HelperOutput {
    pub fn stdout(data: Vec<u8>) -> Self {
        Self::Stdout {
            version: HELPER_PROTOCOL_VERSION,
            data,
        }
    }

    pub fn error(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self::Error {
            version: HELPER_PROTOCOL_VERSION,
            code: code.into(),
            message: message.into(),
        }
    }

    pub fn exit(code: i32) -> Self {
        Self::Exit {
            version: HELPER_PROTOCOL_VERSION,
            code,
        }
    }

    pub fn into_attach_output(self) -> Result<AttachOutput, RuntimeError> {
        let version = match &self {
            Self::Stdout { version, .. }
            | Self::Stderr { version, .. }
            | Self::Error { version, .. }
            | Self::Exit { version, .. } => *version,
        };
        if version != HELPER_PROTOCOL_VERSION {
            return Err(protocol_error(format!(
                "helper protocol version {version} is unsupported; expected {HELPER_PROTOCOL_VERSION}"
            )));
        }
        match self {
            Self::Stdout { data, .. } => Ok(AttachOutput::Stdout(data)),
            Self::Stderr { data, .. } => Ok(AttachOutput::Stderr(data)),
            Self::Exit { code, .. } => Ok(AttachOutput::Exit(code)),
            Self::Error { code, message, .. } => Err(RuntimeError::HelperError {
                operation: "gascan-apple-attach".to_owned(),
                code,
                message,
            }),
        }
    }
}

fn protocol_error(message: String) -> RuntimeError {
    RuntimeError::InvalidOutput {
        operation: "gascan-apple-attach".to_owned(),
        message,
    }
}

mod base64_bytes {
    use super::*;

    pub fn serialize<S>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&STANDARD.encode(bytes))
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Vec<u8>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let encoded = String::deserialize(deserializer)?;
        STANDARD.decode(encoded).map_err(D::Error::custom)
    }
}
