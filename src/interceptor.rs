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

    /// Record or replay a value associated with a stable call-site label.
    ///
    /// Labels make it possible to detect reordered or missing instrumentation
    /// points instead of silently consuming another event's bytes.
    pub fn labeled_value(&mut self, label: &str, value: &[u8]) -> Result<Vec<u8>, Error> {
        let label_bytes = label.as_bytes();
        let label_len = u32::try_from(label_bytes.len())
            .map_err(|_| Error::Process("event label is too long".into()))?;
        let mut payload = Vec::with_capacity(4 + label_bytes.len() + value.len());
        payload.extend_from_slice(&label_len.to_le_bytes());
        payload.extend_from_slice(label_bytes);
        payload.extend_from_slice(value);

        match self {
            Self::Recording(recorder) => {
                recorder.record(EventKind::User, &payload)?;
                Ok(value.to_vec())
            }
            Self::Replaying(cursor) => {
                let recorded = cursor.expect(EventKind::User)?;
                let label_len_bytes = recorded
                    .get(..4)
                    .ok_or_else(|| Error::InvalidTrace("labeled event is truncated".into()))?;
                let recorded_label_len =
                    u32::from_le_bytes(label_len_bytes.try_into().expect("checked length"))
                        as usize;
                let label_end = 4usize
                    .checked_add(recorded_label_len)
                    .ok_or_else(|| Error::InvalidTrace("event label length overflow".into()))?;
                let recorded_label = recorded
                    .get(4..label_end)
                    .ok_or_else(|| Error::InvalidTrace("labeled event is truncated".into()))?;
                if recorded_label != label_bytes {
                    return Err(Error::Process(format!(
                        "event label mismatch: expected `{label}`, trace has `{}`",
                        String::from_utf8_lossy(recorded_label)
                    )));
                }
                Ok(recorded[label_end..].to_vec())
            }
        }
    }

    /// Capture a caller-supplied monotonic tick value, or return its recorded value.
    ///
    /// The caller must read the clock and invoke this at each relevant site;
    /// this method does not hook hardware timers or OS APIs.
    pub fn clock_ticks(&mut self, label: &str, current_ticks: u64) -> Result<u64, Error> {
        let bytes = self.labeled_value(label, &current_ticks.to_le_bytes())?;
        let ticks = bytes
            .as_slice()
            .try_into()
            .map(u64::from_le_bytes)
            .map_err(|_| Error::InvalidTrace("clock event must contain 8 bytes".into()))?;
        Ok(ticks)
    }

    pub fn finish(self) -> Result<(), Error> {
        match self {
            Self::Recording(recorder) => recorder.finish(),
            Self::Replaying(cursor) => cursor.ensure_finished(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_path() -> std::path::PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        std::env::temp_dir().join(format!("paradox-events-{stamp}.trace"))
    }

    #[test]
    fn labeled_values_replay_recorded_data_and_clock_ticks() {
        let path = temp_path();
        let mut recording = EventSession::record_to(&path).expect("create trace");
        assert_eq!(
            recording
                .labeled_value("request-id", b"captured-42")
                .expect("record input"),
            b"captured-42"
        );
        assert_eq!(
            recording
                .clock_ticks("before-wait", 100)
                .expect("record clock"),
            100
        );
        recording.finish().expect("finish recording");

        let mut replay = EventSession::replay_from(&path).expect("open trace");
        assert_eq!(
            replay
                .labeled_value("request-id", b"live-value")
                .expect("replay input"),
            b"captured-42"
        );
        assert_eq!(
            replay
                .clock_ticks("before-wait", 999)
                .expect("replay clock"),
            100
        );
        replay.finish().expect("consume all events");
        std::fs::remove_file(path).expect("remove trace");
    }

    #[test]
    fn labeled_values_reject_a_different_call_site() {
        let path = temp_path();
        let mut recording = EventSession::record_to(&path).expect("create trace");
        recording
            .labeled_value("first-call", b"value")
            .expect("record event");
        recording.finish().expect("finish recording");

        let mut replay = EventSession::replay_from(&path).expect("open trace");
        let error = replay
            .labeled_value("different-call", b"value")
            .expect_err("labels must match");
        assert!(error.to_string().contains("event label mismatch"));
        std::fs::remove_file(path).expect("remove trace");
    }
}
