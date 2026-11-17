//! Deterministic process replay primitives.
//!
//! This crate records explicitly supplied events and process I/O. It does not
//! inject into processes or intercept operating-system calls.

pub mod format;
pub mod interceptor;
pub mod recorder;
pub mod replay;
pub mod storage;

use std::fmt;

/// Errors returned while reading, writing, or replaying a trace.
#[derive(Debug)]
pub enum Error {
    Io(std::io::Error),
    InvalidTrace(String),
    UnexpectedEvent { expected: u8, actual: u8 },
    MissingEvent(&'static str),
    Process(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(f, "I/O error: {error}"),
            Self::InvalidTrace(message) => write!(f, "invalid trace: {message}"),
            Self::UnexpectedEvent { expected, actual } => {
                write!(f, "expected event kind {expected}, found {actual}")
            }
            Self::MissingEvent(name) => write!(f, "trace is missing {name}"),
            Self::Process(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            _ => None,
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

/// Stable event identifiers in the version-1 trace format.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum EventKind {
    Process = 1,
    Stdin = 2,
    Stdout = 3,
    Stderr = 4,
    Exit = 5,
    User = 32,
}

impl EventKind {
    pub fn from_byte(value: u8) -> Result<Self, Error> {
        match value {
            1 => Ok(Self::Process),
            2 => Ok(Self::Stdin),
            3 => Ok(Self::Stdout),
            4 => Ok(Self::Stderr),
            5 => Ok(Self::Exit),
            32 => Ok(Self::User),
            _ => Err(Error::InvalidTrace(format!("unknown event kind {value}"))),
        }
    }
}

/// One event from a trace. Payload bytes are owned by the decoded trace.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TraceEvent {
    pub sequence: u64,
    pub kind: EventKind,
    pub payload: Vec<u8>,
}
