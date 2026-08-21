use crate::mca::access::RandomAccess;
use anyhow::{anyhow, Result};
use byteorder::{ByteOrder, LittleEndian};
use flate2::read::{GzDecoder, ZlibDecoder};
use std::io::Read;
use xxhash_rust::xxh32::xxh32;

pub struct McaEntry {
    access: Box<dyn RandomAccess>,
    start: u64,
    _length: usize,
    index: u32,
    modified: u32,
    region_x: i32,
    region_z: i32,
}

#[derive(Copy, Clone, Eq, PartialEq)]
pub enum CompressionMethod {
    Gzip,
    Zlib,
    Raw,
    Lz4,
    Custom,
    ExternalGzip,
    ExternalZlib,
    ExternalRaw,
    ExternalLz4,
    /// An unrecognized compression method byte. The chunk's raw bytes can still be
    /// preserved, but it cannot be decompressed.
    Unknown,
}

impl McaEntry {
    pub fn new(
        access: Box<dyn RandomAccess>,
        start: u64,
        length: usize,
        index: u32,
        modified: u32,
        region_x: i32,
        region_z: i32,
    ) -> Self {
        Self {
            access,
            start,
            _length: length,
            index,
            modified,
            region_x,
            region_z,
        }
    }
    pub fn region_index(&self) -> u32 {
        self.index
    }
    pub fn x_pos(&self) -> i32 {
        (self.index % 32) as i32
    }
    pub fn z_pos(&self) -> i32 {
        (self.index / 32) as i32
    }
    pub fn global_x(&self) -> i32 {
        self.region_x * 32 + self.x_pos()
    }
    pub fn global_z(&self) -> i32 {
        self.region_z * 32 + self.z_pos()
    }
    pub fn modified_time(&self) -> u32 {
        self.modified
    }

    pub fn read_header(&mut self) -> Result<(u32, CompressionMethod, Option<String>)> {
        self.access.seek(self.start)?;
        let mut buf = [0u8; 5];
        self.access.read_exact(&mut buf)?;
        let len = u32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]]);
        // Guard against a corrupted length field that would make us read huge buffers.
        if len > MAX_VALID_CHUNK_LENGTH {
            return Err(anyhow!("chunk length {} exceeds limit", len));
        }
        let method = buf[4] as i8;
        let cm = match method {
            1 => CompressionMethod::Gzip,
            2 => CompressionMethod::Zlib,
            3 => CompressionMethod::Raw,
            4 => CompressionMethod::Lz4,
            127 => CompressionMethod::Custom,
            -127 => CompressionMethod::ExternalGzip,
            -126 => CompressionMethod::ExternalZlib,
            -125 => CompressionMethod::ExternalRaw,
            -124 => CompressionMethod::ExternalLz4,
            // Unknown method id: do not error here so the raw bytes can still be
            // preserved. Decompression of an Unknown chunk fails (see
            // `all_data_uncompressed`), which the engine treats as "safe keep".
            _ => CompressionMethod::Unknown,
        };
        let mut custom = None;
        if cm == CompressionMethod::Custom {
            let mut lbuf = [0u8; 2];
            self.access.read_exact(&mut lbuf)?;
            let n = u16::from_be_bytes(lbuf) as usize;
            let mut s = vec![0u8; n];
            self.access.read_exact(&mut s)?;
            custom = Some(String::from_utf8_lossy(&s).to_string());
        }
        Ok((len, cm, custom))
    }

    pub fn serialized_bytes(&mut self) -> Result<Vec<u8>> {
        let (len, _, _) = self.read_header()?;
        let total = 4u64 + len as u64;
        self.access.seek(self.start)?;
        let mut out = vec![0u8; total as usize];
        self.access.read_exact(&mut out)?;
        Ok(out)
    }

    pub fn data_bytes(&mut self) -> Result<(CompressionMethod, Vec<u8>, Option<String>)> {
        let (len, cm, custom) = self.read_header()?;
        let mut pos = self.start + 5;
        if cm == CompressionMethod::Custom {
            let mut lbuf = [0u8; 2];
            self.access.read_exact(&mut lbuf)?;
            let n = u16::from_be_bytes(lbuf) as usize;
            let mut s = vec![0u8; n];
            self.access.read_exact(&mut s)?;
            pos += 2 + n as u64;
        }
        let data_len = len as usize
            - 1
            - if cm == CompressionMethod::Custom {
                2 + custom.as_ref().map(|v| v.len()).unwrap_or(0)
            } else {
                0
            };
        self.access.seek(pos)?;
        let mut data = vec![0u8; data_len];
        self.access.read_exact(&mut data)?;
        Ok((cm, data, custom))
    }

    pub fn all_data_uncompressed(&mut self) -> Result<Vec<u8>> {
        let (cm, data, _) = self.data_bytes()?;
        match cm {
            CompressionMethod::Raw => Ok(data),
            CompressionMethod::Zlib => {
                let d = ZlibDecoder::new(&data[..]);
                let mut out = Vec::new();
                // Bound the decompressed output to avoid decompression bombs.
                std::io::copy(
                    &mut d.take(MAX_UNCOMPRESSED_CHUNK_LENGTH as u64 + 1),
                    &mut out,
                )?;
                if out.len() > MAX_UNCOMPRESSED_CHUNK_LENGTH {
                    return Err(anyhow!("zlib decompressed size exceeds limit"));
                }
                Ok(out)
            }
            CompressionMethod::Gzip => {
                let d = GzDecoder::new(&data[..]);
                let mut out = Vec::new();
                std::io::copy(
                    &mut d.take(MAX_UNCOMPRESSED_CHUNK_LENGTH as u64 + 1),
                    &mut out,
                )?;
                if out.len() > MAX_UNCOMPRESSED_CHUNK_LENGTH {
                    return Err(anyhow!("gzip decompressed size exceeds limit"));
                }
                Ok(out)
            }
            CompressionMethod::Lz4 => decode_lz4_blocks(&data),
            // Unknown compression cannot be decompressed; the engine treats the
            // resulting error as "safe keep" (preserve the raw chunk).
            CompressionMethod::Unknown => Err(anyhow!("unknown compression method")),
            // Custom and External* chunks are not decompressible but their raw
            // bytes are preserved; report empty data so callers can decide.
            _ => Ok(Vec::new()),
        }
    }

    pub fn is_external(&mut self) -> Result<bool> {
        let (_, cm, _) = self.read_header()?;
        Ok(matches!(
            cm,
            CompressionMethod::ExternalGzip
                | CompressionMethod::ExternalZlib
                | CompressionMethod::ExternalRaw
                | CompressionMethod::ExternalLz4
        ))
    }
}

const LZ4_MAGIC: &[u8] = b"LZ4Block";
const LZ4_HEADER_LEN: usize = 8 + 1 + 4 + 4 + 4;
const LZ4_XXHASH_SEED: u32 = 0x9747b28c;

/// Upper bound for a chunk's declared serialized length (the 4-byte BE field).
/// Mirrors OrzMCBackup's `MAX_VALID_CHUNK_LENGTH` (8 MB).
pub const MAX_VALID_CHUNK_LENGTH: u32 = 8 * 1024 * 1024;

/// Upper bound for the total decompressed size of a chunk. Guards against
/// decompression bombs. Mirrors OrzMCBackup's `MAX_UNCOMPRESSED_CHUNK_LENGTH`
/// (64 MB).
pub const MAX_UNCOMPRESSED_CHUNK_LENGTH: usize = 64 * 1024 * 1024;

pub fn lz4_checksum(data: &[u8]) -> u32 {
    xxh32(data, LZ4_XXHASH_SEED) & 0x0FFFFFFF
}

pub fn decode_lz4_blocks(inp: &[u8]) -> Result<Vec<u8>> {
    let mut i = 0usize;
    let mut out = Vec::new();
    while i + LZ4_HEADER_LEN <= inp.len() {
        if &inp[i..i + 8] != LZ4_MAGIC {
            return Err(anyhow!("invalid LZ4 magic"));
        }
        let token = inp[i + 8];
        let method = token & 0xF0; // 0x10 RAW, 0x20 LZ4
        let comp_len = LittleEndian::read_u32(&inp[i + 9..i + 13]) as usize;
        let decomp_len = LittleEndian::read_u32(&inp[i + 13..i + 17]) as usize;
        let checksum_le = LittleEndian::read_u32(&inp[i + 17..i + 21]);
        let start = i + LZ4_HEADER_LEN;
        if start + comp_len > inp.len() {
            return Err(anyhow!("LZ4 block truncated"));
        }
        let block = &inp[start..start + comp_len];
        // Validate the declared decompressed length before allocating, to guard
        // against decompression bombs.
        if decomp_len > MAX_UNCOMPRESSED_CHUNK_LENGTH {
            return Err(anyhow!(
                "LZ4 decompressed length {} exceeds limit",
                decomp_len
            ));
        }
        let decoded = if method == 0x10 {
            // RAW
            block.to_vec()
        } else if method == 0x20 {
            // LZ4
            let mut with_prepended = Vec::with_capacity(4 + block.len());
            let mut size_buf = [0u8; 4];
            LittleEndian::write_u32(&mut size_buf, decomp_len as u32);
            with_prepended.extend_from_slice(&size_buf);
            with_prepended.extend_from_slice(block);
            lz4_flex::block::decompress_size_prepended(&with_prepended)?
        } else {
            return Err(anyhow!("unsupported LZ4 method"));
        };
        let checksum = lz4_checksum(&decoded);
        if checksum != checksum_le {
            return Err(anyhow!("LZ4 checksum mismatch"));
        }
        if out.len().saturating_add(decoded.len()) > MAX_UNCOMPRESSED_CHUNK_LENGTH {
            return Err(anyhow!("LZ4 cumulative decompressed size exceeds limit"));
        }
        out.extend_from_slice(&decoded);
        i = start + comp_len;
    }
    if i != inp.len() {
        return Err(anyhow!("dangling LZ4 bytes"));
    }
    Ok(out)
}
