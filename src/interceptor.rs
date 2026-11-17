//! Safe, explicit instrumentation boundary for applications built with Paradox.
//!
//! This is an application-level event source, not an OS API hook. The caller
//! supplies each value it wants recorded and consumes the matching value later.

use crate::recorder::Recorder;
use crate::replay::ReplayCursor;
use crate::{Error, EventKind};
use std::path::Path;

pub enum EventSession {
    Recording(Recorder),
    Replaying(ReplayCursor),
}

impl EventSession {
    pub fn record_to(path: impl AsRef<Path>) -> Result<Self, Error> {
        Ok(Self::Recording(Recorder::create(path)?))
    }

    pub fn replay_from(path: impl AsRef<Path>) -> Result<Self, Error> {
        Ok(Self::Replaying(ReplayCursor::open(path)?))
    }

    /// Record an explicitly supplied value, or return the next recorded value.
    pub fn nondeterministic_value(&mut self, value: &[u8]) -> Result<Vec<u8>, Error> {
        match self {
            Self::Recording(recorder) => {
                recorder.record(EventKind::User, value)?;
                Ok(value.to_vec())
            }
            Self::Replaying(cursor) => cursor.expect(EventKind::User),
        }
    }

    pub fn finish(self) -> Result<(), Error> {
        match self {
            Self::Recording(recorder) => recorder.finish(),
            Self::Replaying(cursor) => cursor.ensure_finished(),
        }
    }
}
