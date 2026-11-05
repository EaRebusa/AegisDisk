#![allow(dead_code, unused_imports)]
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct MftDeletedFile {
    pub record_number: u64,
    pub filename: String,
    pub file_size: u64,
    pub is_directory: bool,
    pub resident_data: Option<Vec<u8>>,
    pub data_runs: Vec<MftDataRun>,
}

#[derive(Debug, Clone)]
pub struct MftDataRun {
    pub cluster_count: u64,
    pub lcn_offset: i64,
}

pub const MFT_RECORD_MAGIC: &[u8; 4] = b"FILE";
pub const MFT_RECORD_SIZE: usize = 1024;

/// Parse a single 1024-byte NTFS MFT Record
pub fn parse_mft_record(raw: &[u8], record_number: u64) -> Option<MftDeletedFile> {
    if raw.len() < MFT_RECORD_SIZE || &raw[..4] != MFT_RECORD_MAGIC {
        return None;
    }

    // Fixup update sequence validation and application
    let mut record = raw[..MFT_RECORD_SIZE].to_vec();
    if !apply_fixups(&mut record) {
        return None;
    }

    let flags = u16::from_le_bytes([record[0x16], record[0x17]]);
    let is_in_use = (flags & 0x0001) != 0;
    let is_directory = (flags & 0x0002) != 0;

    // We specifically target deleted files (in-use flag is false)
    if is_in_use {
        return None;
    }

    let first_attr_offset = u16::from_le_bytes([record[0x14], record[0x15]]) as usize;
    if first_attr_offset >= record.len() {
        return None;
    }

    let mut filename = format!("deleted_record_{}", record_number);
    let mut file_size = 0u64;
    let mut resident_data = None;
    let mut data_runs = Vec::new();

    let mut attr_offset = first_attr_offset;
    while attr_offset + 8 <= record.len() {
        let attr_type = u32::from_le_bytes([
            record[attr_offset],
            record[attr_offset + 1],
            record[attr_offset + 2],
            record[attr_offset + 3],
        ]);

        if attr_type == 0xFFFFFFFF {
            // End of attributes marker
            break;
        }

        let attr_len = u32::from_le_bytes([
            record[attr_offset + 4],
            record[attr_offset + 5],
            record[attr_offset + 6],
            record[attr_offset + 7],
        ]) as usize;

        if attr_len == 0 || attr_offset + attr_len > record.len() {
            break;
        }

        let is_non_resident = record[attr_offset + 8] != 0;

        match attr_type {
            0x30 => {
                // $FILE_NAME
                if !is_non_resident && attr_offset + 24 <= record.len() {
                    let content_offset = u16::from_le_bytes([record[attr_offset + 20], record[attr_offset + 21]]) as usize;
                    let content_start = attr_offset + content_offset;
                    if content_start + 66 <= record.len() {
                        let name_len = record[content_start + 64] as usize;
                        let name_start = content_start + 66;
                        if name_start + (name_len * 2) <= record.len() {
                            let utf16_units: Vec<u16> = record[name_start..name_start + (name_len * 2)]
                                .chunks_exact(2)
                                .map(|c| u16::from_le_bytes([c[0], c[1]]))
                                .collect();
                            if let Ok(parsed_name) = String::from_utf16(&utf16_units) {
                                filename = parsed_name;
                            }
                        }
                    }
                }
            }
            0x80 => {
                // $DATA
                if is_non_resident {
                    if attr_offset + 0x38 <= record.len() {
                        file_size = u64::from_le_bytes(
                            record[attr_offset + 0x30..attr_offset + 0x38]
                                .try_into()
                                .unwrap_or([0; 8]),
                        );
                        let runlist_offset = u16::from_le_bytes([record[attr_offset + 0x20], record[attr_offset + 0x21]]) as usize;
                        data_runs = decode_data_runs(&record[attr_offset + runlist_offset..attr_offset + attr_len]);
                    }
                } else {
                    let content_len = u32::from_le_bytes([
                        record[attr_offset + 0x10],
                        record[attr_offset + 0x11],
                        record[attr_offset + 0x12],
                        record[attr_offset + 0x13],
                    ]) as usize;
                    let content_offset = u16::from_le_bytes([record[attr_offset + 0x14], record[attr_offset + 0x15]]) as usize;
                    let data_start = attr_offset + content_offset;
                    if data_start + content_len <= record.len() {
                        file_size = content_len as u64;
                        resident_data = Some(record[data_start..data_start + content_len].to_vec());
                    }
                }
            }
            _ => {}
        }

        attr_offset += attr_len;
    }

    Some(MftDeletedFile {
        record_number,
        filename,
        file_size,
        is_directory,
        resident_data,
        data_runs,
    })
}

fn apply_fixups(record: &mut [u8]) -> bool {
    let fixup_offset = u16::from_le_bytes([record[4], record[5]]) as usize;
    let fixup_count = u16::from_le_bytes([record[6], record[7]]) as usize;

    if fixup_count == 0 || fixup_offset + (fixup_count * 2) > record.len() {
        return false;
    }

    let usn = u16::from_le_bytes([record[fixup_offset], record[fixup_offset + 1]]);

    for i in 1..fixup_count {
        let sector_end = (i * 512) - 2;
        if sector_end + 2 > record.len() {
            break;
        }

        let sector_usn = u16::from_le_bytes([record[sector_end], record[sector_end + 1]]);
        if sector_usn != usn {
            // USN check failed: sector corrupted
            return false;
        }

        // Restore original bytes
        let orig_bytes_offset = fixup_offset + (i * 2);
        record[sector_end] = record[orig_bytes_offset];
        record[sector_end + 1] = record[orig_bytes_offset + 1];
    }

    true
}

fn decode_data_runs(mut raw: &[u8]) -> Vec<MftDataRun> {
    let mut runs = Vec::new();
    let mut current_lcn = 0i64;

    while !raw.is_empty() {
        let header = raw[0];
        if header == 0 {
            break;
        }
        raw = &raw[1..];

        let len_bytes = (header & 0x0F) as usize;
        let offset_bytes = ((header >> 4) & 0x0F) as usize;

        if raw.len() < len_bytes + offset_bytes {
            break;
        }

        let mut count_buf = [0u8; 8];
        count_buf[..len_bytes].copy_from_slice(&raw[..len_bytes]);
        let cluster_count = u64::from_le_bytes(count_buf);
        raw = &raw[len_bytes..];

        let mut offset_buf = [0u8; 8];
        offset_buf[..offset_bytes].copy_from_slice(&raw[..offset_bytes]);
        // Sign extend the LCN delta
        let mut lcn_delta = i64::from_le_bytes(offset_buf);
        if offset_bytes > 0 && (raw[offset_bytes - 1] & 0x80) != 0 {
            for b in offset_bytes..8 {
                lcn_delta |= (0xFF as i64) << (b * 8);
            }
        }
        raw = &raw[offset_bytes..];

        current_lcn += lcn_delta;
        runs.push(MftDataRun {
            cluster_count,
            lcn_offset: current_lcn,
        });
    }

    runs
}
