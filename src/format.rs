//! Stable, little-endian trace encoding.
//!
//! The format is framed and versioned. It is intentionally decoded into owned
//! records in this first version; memory mapping and zero-copy views are not
//! implemented.

use crate::{Error, EventKind, TraceEvent};
use std::io::{Read, Write};

pub const MAGIC: &[u8; 8] = b"PARADXTR";
pub const TRAILER: &[u8; 8] = b"PARADEND";
pub const VERSION: u16 = 1;
pub const MAX_RECORD_BYTES: usize = 64 * 1024 * 1024;

pub fn write_header<W: Write>(writer: &mut W) -> Result<(), Error> {
    writer.write_all(MAGIC)?;
    writer.write_all(&VERSION.to_le_bytes())?;
    writer.write_all(&0u16.to_le_bytes())?;
    Ok(())
}

pub fn read_header<R: Read>(reader: &mut R) -> Result<(), Error> {
    let mut magic = [0u8; 8];
    reader.read_exact(&mut magic)?;
    if &magic != MAGIC {
        return Err(Error::InvalidTrace("bad magic bytes".into()));
    }
    let version = read_u16(reader)?;
    if version != VERSION {
        return Err(Error::InvalidTrace(format!(
            "unsupported version {version}; this build supports {VERSION}"
        )));
    }
    let _flags = read_u16(reader)?;
    Ok(())
}

pub fn write_event<W: Write>(writer: &mut W, event: &TraceEvent) -> Result<(), Error> {
    if event.payload.len() > MAX_RECORD_BYTES {
        return Err(Error::InvalidTrace(format!(
            "event payload exceeds {MAX_RECORD_BYTES} bytes"
        )));
    }
    let length = event.payload.len() as u32;
    let checksum = checksum(event.kind as u8, event.sequence, &event.payload);
    writer.write_all(&[event.kind as u8, 0, 0, 0])?;
    writer.write_all(&event.sequence.to_le_bytes())?;
    writer.write_all(&length.to_le_bytes())?;
    writer.write_all(&event.payload)?;
    writer.write_all(&checksum.to_le_bytes())?;
    Ok(())
}

/// Returns `None` only when the stream ends cleanly after the trailer.
pub fn read_event<R: Read>(
    reader: &mut R,
    expected_sequence: u64,
) -> Result<Option<TraceEvent>, Error> {
    let mut first = [0u8; 1];
    match reader.read(&mut first)? {
        0 => return Err(Error::InvalidTrace("missing end-of-trace trailer".into())),
        1 => {}
        _ => unreachable!(),
    }

    if first[0] == TRAILER[0] {
        let mut rest = [0u8; 7];
        reader.read_exact(&mut rest)?;
        if rest == TRAILER[1..] {
            let count = read_u64(reader)?;
            if count != expected_sequence {
                return Err(Error::InvalidTrace(format!(
                    "trailer says {count} events, decoded {expected_sequence}"
                )));
            }
            return Ok(None);
        }
        return Err(Error::InvalidTrace("unexpected trailer marker".into()));
    }

    let kind = EventKind::from_byte(first[0])?;
    let mut reserved = [0u8; 3];
    reader.read_exact(&mut reserved)?;
    if reserved != [0, 0, 0] {
        return Err(Error::InvalidTrace(
            "reserved frame bits must be zero".into(),
        ));
    }
    let sequence = read_u64(reader)?;
    if sequence != expected_sequence {
        return Err(Error::InvalidTrace(format!(
            "expected sequence {expected_sequence}, found {sequence}"
        )));
    }
    let length = read_u32(reader)? as usize;
    if length > MAX_RECORD_BYTES {
        return Err(Error::InvalidTrace(format!(
            "event payload exceeds {MAX_RECORD_BYTES} bytes"
        )));
    }
    let mut payload = vec![0; length];
    reader.read_exact(&mut payload)?;
    let stored_checksum = read_u64(reader)?;
    if stored_checksum != checksum(kind as u8, sequence, &payload) {
        return Err(Error::InvalidTrace(format!(
            "checksum mismatch at event {sequence}"
        )));
    }
    Ok(Some(TraceEvent {
        sequence,
        kind,
        payload,
    }))
}

pub fn write_trailer<W: Write>(writer: &mut W, count: u64) -> Result<(), Error> {
    writer.write_all(TRAILER)?;
    writer.write_all(&count.to_le_bytes())?;
    Ok(())
}

fn checksum(kind: u8, sequence: u64, payload: &[u8]) -> u64 {
    // FNV-1a detects accidental corruption; it is not a cryptographic MAC.
    let mut hash = 0xcbf29ce484222325u64;
    for byte in [kind]
        .into_iter()
        .chain(sequence.to_le_bytes())
        .chain((payload.len() as u32).to_le_bytes())
        .chain(payload.iter().copied())
    {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

fn read_u16<R: Read>(reader: &mut R) -> Result<u16, Error> {
    let mut bytes = [0; 2];
    reader.read_exact(&mut bytes)?;
    Ok(u16::from_le_bytes(bytes))
}

fn read_u32<R: Read>(reader: &mut R) -> Result<u32, Error> {
    let mut bytes = [0; 4];
    reader.read_exact(&mut bytes)?;
    Ok(u32::from_le_bytes(bytes))
}

fn read_u64<R: Read>(reader: &mut R) -> Result<u64, Error> {
    let mut bytes = [0; 8];
    reader.read_exact(&mut bytes)?;
    Ok(u64::from_le_bytes(bytes))
}
