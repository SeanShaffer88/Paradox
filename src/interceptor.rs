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
                recorder.record(EventKind::LabeledUser, &payload)?;
                Ok(value.to_vec())
            }
            Self::Replaying(cursor) => {
                let recorded = cursor.expect(EventKind::LabeledUser)?;
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

    /// Record or verify an application-provided thread-order marker.
    ///
    /// This detects an order/thread mismatch; it does not pause or schedule threads.
    pub fn thread_marker(&mut self, label: &str, thread_id: u64) -> Result<(), Error> {
        let bytes = self.labeled_value(label, &thread_id.to_le_bytes())?;
        let recorded = bytes
            .as_slice()
            .try_into()
            .map(u64::from_le_bytes)
            .map_err(|_| Error::InvalidTrace("thread marker must contain 8 bytes".into()))?;
        if recorded != thread_id {
            return Err(Error::Process(format!(
                "thread marker `{label}` differs: recorded {recorded}, replayed {thread_id}"
            )));
        }
        Ok(())
    }

    /// Record a delta for an app-supplied stable location ID.
    ///
    /// Replay checks the location and prior bytes, then returns the recorded
    /// post-state. The library never reads or writes process memory itself.
    pub fn memory_delta(
        &mut self,
        label: &str,
        location_id: u64,
        before: &[u8],
        after: &[u8],
    ) -> Result<Vec<u8>, Error> {
        let payload = encode_memory_delta(location_id, before, after)?;
        let recorded = self.labeled_value(label, &payload)?;
        let (recorded_location, recorded_before, recorded_after) = decode_memory_delta(&recorded)?;
        if recorded_location != location_id {
            return Err(Error::Process(format!(
                "memory location mismatch at `{label}`: recorded {recorded_location}, replayed {location_id}"
            )));
        }
        if recorded_before != before {
            return Err(Error::Process(format!(
                "memory pre-state mismatch at `{label}`"
            )));
        }
        Ok(recorded_after.to_vec())
    }

    pub fn finish(self) -> Result<(), Error> {
        match self {
            Self::Recording(recorder) => recorder.finish(),
            Self::Replaying(cursor) => cursor.ensure_finished(),
        }
    }
}

fn encode_memory_delta(location_id: u64, before: &[u8], after: &[u8]) -> Result<Vec<u8>, Error> {
    let before_len = u32::try_from(before.len())
        .map_err(|_| Error::Process("memory pre-state is too large".into()))?;
    let after_len = u32::try_from(after.len())
        .map_err(|_| Error::Process("memory post-state is too large".into()))?;
    let mut payload = Vec::with_capacity(16 + before.len() + after.len());
    payload.extend_from_slice(&location_id.to_le_bytes());
    payload.extend_from_slice(&before_len.to_le_bytes());
    payload.extend_from_slice(before);
    payload.extend_from_slice(&after_len.to_le_bytes());
    payload.extend_from_slice(after);
    Ok(payload)
}

fn decode_memory_delta(bytes: &[u8]) -> Result<(u64, &[u8], &[u8]), Error> {
    let mut reader = DeltaReader { bytes, offset: 0 };
    let location_id = reader.u64()?;
    let before = reader.field()?;
    let after = reader.field()?;
    if reader.offset != bytes.len() {
        return Err(Error::InvalidTrace(
            "unexpected bytes after memory delta".into(),
        ));
    }
    Ok((location_id, before, after))
}

struct DeltaReader<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> DeltaReader<'a> {
    fn take(&mut self, length: usize) -> Result<&'a [u8], Error> {
        let end = self
            .offset
            .checked_add(length)
            .ok_or_else(|| Error::InvalidTrace("memory delta length overflow".into()))?;
        let field = self
            .bytes
            .get(self.offset..end)
            .ok_or_else(|| Error::InvalidTrace("truncated memory delta".into()))?;
        self.offset = end;
        Ok(field)
    }

    fn u32(&mut self) -> Result<u32, Error> {
        Ok(u32::from_le_bytes(
            self.take(4)?.try_into().expect("checked length"),
        ))
    }

    fn u64(&mut self) -> Result<u64, Error> {
        Ok(u64::from_le_bytes(
            self.take(8)?.try_into().expect("checked length"),
        ))
    }

    fn field(&mut self) -> Result<&'a [u8], Error> {
        let length = self.u32()? as usize;
        self.take(length)
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

    #[test]
    fn replays_thread_markers_and_memory_deltas() {
        let path = temp_path();
        let mut recording = EventSession::record_to(&path).expect("create trace");
        recording
            .thread_marker("worker-start", 7)
            .expect("record thread");
        assert_eq!(
            recording
                .memory_delta("counter", 3, &[0], &[1])
                .expect("record delta"),
            [1]
        );
        recording.finish().expect("finish recording");

        let mut replay = EventSession::replay_from(&path).expect("open trace");
        replay
            .thread_marker("worker-start", 7)
            .expect("replay thread");
        assert_eq!(
            replay
                .memory_delta("counter", 3, &[0], &[9])
                .expect("replay delta"),
            [1]
        );
        replay.finish().expect("consume all events");
        std::fs::remove_file(path).expect("remove trace");
    }

    #[test]
    fn memory_delta_rejects_a_different_prior_state() {
        let path = temp_path();
        let mut recording = EventSession::record_to(&path).expect("create trace");
        recording
            .memory_delta("counter", 3, &[0], &[1])
            .expect("record delta");
        recording.finish().expect("finish recording");

        let mut replay = EventSession::replay_from(&path).expect("open trace");
        let error = replay
            .memory_delta("counter", 3, &[2], &[9])
            .expect_err("pre-state must match");
        assert!(error.to_string().contains("memory pre-state mismatch"));
        std::fs::remove_file(path).expect("remove trace");
    }

    #[test]
    fn raw_and_labeled_events_cannot_be_confused() {
        let path = temp_path();
        let mut recording = EventSession::record_to(&path).expect("create trace");
        recording
            .nondeterministic_value(b"raw")
            .expect("record raw event");
        recording.finish().expect("finish recording");

        let mut replay = EventSession::replay_from(&path).expect("open trace");
        let error = replay
            .labeled_value("call-site", b"live")
            .expect_err("event kinds must differ");
        assert!(error
            .to_string()
            .contains("expected event kind 33, found 32"));
        std::fs::remove_file(path).expect("remove trace");
    }
}
