//! Trace cursor and process replay helpers.

use crate::storage::read_trace;
use crate::{Error, EventKind, TraceEvent};
use std::path::Path;

pub struct ReplayCursor {
    events: Vec<TraceEvent>,
    position: usize,
}

impl ReplayCursor {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, Error> {
        Ok(Self {
            events: read_trace(path)?,
            position: 0,
        })
    }

    pub fn position(&self) -> usize {
        self.position
    }

    pub fn len(&self) -> usize {
        self.events.len()
    }

    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    pub fn current(&self) -> Option<&TraceEvent> {
        self.position
            .checked_sub(1)
            .and_then(|index| self.events.get(index))
    }

    pub fn step_forward(&mut self) -> Option<&TraceEvent> {
        let event = self.events.get(self.position)?;
        self.position += 1;
        Some(event)
    }

    pub fn step_back(&mut self) -> Option<&TraceEvent> {
        if self.position == 0 {
            return None;
        }
        self.position -= 1;
        self.events.get(self.position)
    }

    pub fn expect(&mut self, expected: EventKind) -> Result<Vec<u8>, Error> {
        let event = self
            .events
            .get(self.position)
            .ok_or(Error::MissingEvent("requested event"))?;
        if event.kind != expected {
            return Err(Error::UnexpectedEvent {
                expected: expected as u8,
                actual: event.kind as u8,
            });
        }
        let payload = event.payload.clone();
        self.position += 1;
        Ok(payload)
    }

    pub fn ensure_finished(self) -> Result<(), Error> {
        if self.position == self.events.len() {
            Ok(())
        } else {
            Err(Error::Process(format!(
                "{} recorded events were not consumed",
                self.events.len() - self.position
            )))
        }
    }

    pub fn events(&self) -> &[TraceEvent] {
        &self.events
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recorder::Recorder;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_path() -> std::path::PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        std::env::temp_dir().join(format!("paradox-replay-{stamp}.trace"))
    }

    #[test]
    fn cursor_steps_both_directions_and_replays_values() {
        let path = temp_path();
        let mut recorder = Recorder::create(&path).expect("create trace");
        recorder
            .record(EventKind::User, b"first")
            .expect("record first");
        recorder
            .record(EventKind::User, b"second")
            .expect("record second");
        recorder.finish().expect("finish trace");

        let mut cursor = ReplayCursor::open(&path).expect("open trace");
        assert_eq!(
            cursor.expect(EventKind::User).expect("first value"),
            b"first"
        );
        assert_eq!(cursor.position(), 1);
        assert_eq!(cursor.step_back().expect("step back").payload, b"first");
        assert_eq!(
            cursor.step_forward().expect("step forward").payload,
            b"first"
        );
        assert_eq!(
            cursor.expect(EventKind::User).expect("second value"),
            b"second"
        );
        cursor.ensure_finished().expect("all events consumed");
        std::fs::remove_file(path).expect("remove trace");
    }
}
