use crate::mca::entry::McaEntry;
use crate::patterns::ChunkPattern;
use anyhow::Result;
use byteorder::{BigEndian, ByteOrder};

pub struct InhabitedTimePattern {
    threshold: i64,
    remove_unknown: bool,
}

impl InhabitedTimePattern {
    pub fn new(threshold: i64, remove_unknown: bool) -> Self {
        Self {
            threshold,
            remove_unknown,
        }
    }
}

const LONG_TAG: u8 = 4;
fn find_inhabited_fast(data: &[u8]) -> Option<i64> {
    let name = b"InhabitedTime";
    let mut prefix = Vec::with_capacity(1 + 2 + name.len());
    prefix.push(LONG_TAG);
    let mut lenbuf = [0u8; 2];
    BigEndian::write_u16(&mut lenbuf, name.len() as u16);
    prefix.extend_from_slice(&lenbuf);
    prefix.extend_from_slice(name);
    let plen = prefix.len();
    if data.len() < plen + 8 {
        return None;
    }
    let mut i = 0usize;
    while i + plen + 8 <= data.len() {
        if data[i..i + plen] == prefix[..] {
            let v = BigEndian::read_i64(&data[i + plen..i + plen + 8]);
            return Some(v);
        }
        i += 1;
    }
    None
}

impl ChunkPattern for InhabitedTimePattern {
    fn matches(&self, entry: &mut McaEntry) -> Result<bool> {
        if entry.is_external()? {
            return Ok(!self.remove_unknown);
        }
        let de = entry.all_data_uncompressed()?;
        if de.is_empty() {
            return Ok(!self.remove_unknown);
        }
        if let Some(t) = find_inhabited_fast(&de) {
            // Strictly greater: a chunk whose InhabitedTime equals the threshold is removed.
            // Matches Aternos/Thanos (InhabitedTimePattern.php) and OrzMCBackup semantics.
            return Ok(t > self.threshold);
        }
        Ok(!self.remove_unknown)
    }

    fn box_clone(&self) -> Box<dyn ChunkPattern> {
        Box::new(Self {
            threshold: self.threshold,
            remove_unknown: self.remove_unknown,
        })
    }
}
