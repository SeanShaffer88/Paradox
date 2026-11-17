//! Trace file reader and writer.

use crate::format;
use crate::{Error, TraceEvent};
use std::fs::File;
use std::io::{BufReader, BufWriter, Write};
use std::path::Path;

pub struct TraceWriter {
    writer: BufWriter<File>,
    count: u64,
}

impl TraceWriter {
    pub fn create(path: impl AsRef<Path>) -> Result<Self, Error> {
        let file = File::create(path)?;
        let mut writer = BufWriter::new(file);
        format::write_header(&mut writer)?;
        Ok(Self { writer, count: 0 })
    }

    pub fn push(&mut self, event: &TraceEvent) -> Result<(), Error> {
        if event.sequence != self.count {
            return Err(Error::InvalidTrace(format!(
                "writer expected event {}, received {}",
                self.count, event.sequence
            )));
        }
        format::write_event(&mut self.writer, event)?;
        self.count += 1;
        Ok(())
    }

    pub fn finish(mut self) -> Result<(), Error> {
        format::write_trailer(&mut self.writer, self.count)?;
        self.writer.flush()?;
        Ok(())
    }
}

pub fn read_trace(path: impl AsRef<Path>) -> Result<Vec<TraceEvent>, Error> {
    let mut reader = BufReader::new(File::open(path)?);
    format::read_header(&mut reader)?;
    let mut events = Vec::new();
    while let Some(event) = format::read_event(&mut reader, events.len() as u64)? {
        events.push(event);
    }
    let mut trailing = [0u8; 1];
    if reader.read(&mut trailing)? != 0 {
        return Err(Error::InvalidTrace(
            "bytes found after end-of-trace trailer".into(),
        ));
    }
    Ok(events)
}

use std::io::Read;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EventKind, TraceEvent};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_path() -> std::path::PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        std::env::temp_dir().join(format!("paradox-storage-{stamp}.trace"))
    }

    #[test]
    fn rejects_a_corrupted_payload() {
        let path = temp_path();
        let mut writer = TraceWriter::create(&path).expect("create trace");
        writer
            .push(&TraceEvent {
                sequence: 0,
                kind: EventKind::User,
                payload: b"payload".to_vec(),
            })
            .expect("write event");
        writer.finish().expect("finish trace");

        let mut bytes = std::fs::read(&path).expect("read trace");
        bytes[28] ^= 0x01;
        std::fs::write(&path, bytes).expect("corrupt trace");
        let error = read_trace(&path).expect_err("checksum should fail");
        assert!(error.to_string().contains("checksum mismatch"));
        std::fs::remove_file(path).expect("remove trace");
    }
}
