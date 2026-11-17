//! Explicit event recorder. Callers decide which deterministic inputs to log.

use crate::storage::TraceWriter;
use crate::{Error, EventKind, TraceEvent};
use std::path::Path;

pub struct Recorder {
    writer: TraceWriter,
    next_sequence: u64,
}

impl Recorder {
    pub fn create(path: impl AsRef<Path>) -> Result<Self, Error> {
        Ok(Self {
            writer: TraceWriter::create(path)?,
            next_sequence: 0,
        })
    }

    pub fn create_overwrite(path: impl AsRef<Path>) -> Result<Self, Error> {
        Ok(Self {
            writer: TraceWriter::create_overwrite(path)?,
            next_sequence: 0,
        })
    }

    pub fn record(&mut self, kind: EventKind, payload: &[u8]) -> Result<u64, Error> {
        let sequence = self.next_sequence;
        self.writer.push(&TraceEvent {
            sequence,
            kind,
            payload: payload.to_vec(),
        })?;
        self.next_sequence += 1;
        Ok(sequence)
    }

    pub fn finish(self) -> Result<(), Error> {
        self.writer.finish()
    }
}
