use std::time::{SystemTime, UNIX_EPOCH};
use crc32fast::Hasher;

pub const PROBE_MAGIC: &[u8; 8] = b"AEGISPRB";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PatternVerification {
    Valid,
    WrapAroundOverwrite { written_for_offset: u64 },
    BlackHoleZeros,
    BlackHoleOnes,
    CrcMismatch { expected: u32, found: u32 },
    InvalidMagic,
}

pub fn generate_probe_block(buffer: &mut [u8], byte_offset: u64) {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    // 0..8: Magic
    buffer[..8].copy_from_slice(PROBE_MAGIC);
    // 8..16: Target byte offset
    buffer[8..16].copy_from_slice(&byte_offset.to_le_bytes());
    // 16..24: Timestamp
    buffer[16..24].copy_from_slice(&now.to_le_bytes());
    // 24..32: Target LBA (assuming 512b)
    let lba = byte_offset / 512;
    buffer[24..32].copy_from_slice(&lba.to_le_bytes());

    // Compute CRC32 of first 32 bytes
    let mut hasher = Hasher::new();
    hasher.update(&buffer[..32]);
    let crc = hasher.finalize();

    // 32..36: CRC32
    buffer[32..36].copy_from_slice(&crc.to_le_bytes());

    // 36..len: Deterministic payload based on offset
    let mut state = byte_offset ^ 0xDEADBEEFCAFEBABE;
    for chunk in buffer[36..].chunks_exact_mut(8) {
        state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
        chunk.copy_from_slice(&state.to_le_bytes());
    }
}

pub fn verify_probe_block(buffer: &[u8], expected_offset: u64) -> PatternVerification {
    if buffer.len() < 36 {
        return PatternVerification::InvalidMagic;
    }

    // Check for black-hole drops (all zeros or all 0xFF)
    if buffer.iter().all(|&b| b == 0) {
        return PatternVerification::BlackHoleZeros;
    }
    if buffer.iter().all(|&b| b == 0xFF) {
        return PatternVerification::BlackHoleOnes;
    }

    if &buffer[..8] != PROBE_MAGIC {
        return PatternVerification::InvalidMagic;
    }

    // Check CRC
    let mut hasher = Hasher::new();
    hasher.update(&buffer[..32]);
    let computed_crc = hasher.finalize();

    let recorded_crc = u32::from_le_bytes([buffer[32], buffer[33], buffer[34], buffer[35]]);
    if computed_crc != recorded_crc {
        return PatternVerification::CrcMismatch {
            expected: computed_crc,
            found: recorded_crc,
        };
    }

    let recorded_offset = u64::from_le_bytes(buffer[8..16].try_into().unwrap_or([0; 8]));
    if recorded_offset != expected_offset {
        return PatternVerification::WrapAroundOverwrite {
            written_for_offset: recorded_offset,
        };
    }

    PatternVerification::Valid
}
