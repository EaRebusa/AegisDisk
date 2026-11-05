#![allow(dead_code, unused_imports)]
use std::path::{Path, PathBuf};

/// EXT4 Superblock Constants
pub const EXT4_SUPERBLOCK_OFFSET: u64 = 1024;
pub const EXT4_SUPERBLOCK_SIZE: usize = 1024;
pub const EXT4_SUPERBLOCK_MAGIC: u16 = 0xEF53;

#[derive(Debug, Clone)]
pub struct Ext4Superblock {
    pub inodes_count: u32,
    pub blocks_count: u64,
    pub free_blocks_count: u64,
    pub free_inodes_count: u32,
    pub first_data_block: u32,
    pub block_size: u32,
    pub inode_size: u16,
    pub blocks_per_group: u32,
    pub inodes_per_group: u32,
    pub volume_name: String,
}

#[derive(Debug, Clone)]
pub struct Ext4Extent {
    pub logical_block: u32,
    pub block_count: u16,
    pub physical_block: u64,
    pub is_unwritten: bool,
}

#[derive(Debug, Clone)]
pub struct Ext4Inode {
    pub inode_number: u32,
    pub mode: u16,
    pub size_bytes: u64,
    pub is_directory: bool,
    pub is_deleted: bool,
    pub extents: Vec<Ext4Extent>,
}

#[derive(Debug, Clone)]
pub struct Ext4DirEntry {
    pub inode: u32,
    pub file_type: u8,
    pub name: String,
}

impl Ext4Superblock {
    pub fn parse(raw: &[u8]) -> Option<Self> {
        if raw.len() < 1024 {
            return None;
        }

        let magic = u16::from_le_bytes([raw[0x38], raw[0x39]]);
        if magic != EXT4_SUPERBLOCK_MAGIC {
            return None;
        }

        let inodes_count = u32::from_le_bytes(raw[0x00..0x04].try_into().unwrap_or([0; 4]));
        let blocks_lo = u32::from_le_bytes(raw[0x04..0x08].try_into().unwrap_or([0; 4])) as u64;
        let blocks_hi = u32::from_le_bytes(raw[0x150..0x154].try_into().unwrap_or([0; 4])) as u64;
        let blocks_count = (blocks_hi << 32) | blocks_lo;

        let free_blocks_lo = u32::from_le_bytes(raw[0x0C..0x10].try_into().unwrap_or([0; 4])) as u64;
        let free_blocks_hi = u32::from_le_bytes(raw[0x158..0x15C].try_into().unwrap_or([0; 4])) as u64;
        let free_blocks_count = (free_blocks_hi << 32) | free_blocks_lo;

        let free_inodes_count = u32::from_le_bytes(raw[0x10..0x14].try_into().unwrap_or([0; 4]));
        let first_data_block = u32::from_le_bytes(raw[0x14..0x18].try_into().unwrap_or([0; 4]));

        let log_block_size = u32::from_le_bytes(raw[0x18..0x1C].try_into().unwrap_or([0; 4]));
        let block_size = 1024u32.checked_shl(log_block_size).unwrap_or(4096);

        let blocks_per_group = u32::from_le_bytes(raw[0x20..0x24].try_into().unwrap_or([0; 4]));
        let inodes_per_group = u32::from_le_bytes(raw[0x28..0x2C].try_into().unwrap_or([0; 4]));

        let mut inode_size = u16::from_le_bytes([raw[0x58], raw[0x59]]);
        if inode_size == 0 {
            inode_size = 128; // Ext2 standard fallback
        }

        let vol_bytes = &raw[0x78..0x88];
        let volume_name = String::from_utf8_lossy(vol_bytes).trim_matches('\0').trim().to_string();

        Some(Self {
            inodes_count,
            blocks_count,
            free_blocks_count,
            free_inodes_count,
            first_data_block,
            block_size,
            inode_size,
            blocks_per_group,
            inodes_per_group,
            volume_name,
        })
    }
}

impl Ext4Inode {
    pub fn parse(raw: &[u8], inode_number: u32) -> Option<Self> {
        if raw.len() < 128 {
            return None;
        }

        let mode = u16::from_le_bytes([raw[0x00], raw[0x01]]);
        let is_directory = (mode & 0xF000) == 0x4000;
        let _is_regular_file = (mode & 0xF000) == 0x8000;

        let dtime = u32::from_le_bytes(raw[0x14..0x18].try_into().unwrap_or([0; 4]));
        let links_count = u16::from_le_bytes([raw[0x1A], raw[0x1B]]);
        let is_deleted = dtime > 0 || links_count == 0;

        let size_lo = u32::from_le_bytes(raw[0x04..0x08].try_into().unwrap_or([0; 4])) as u64;
        let size_hi = if !is_directory && raw.len() >= 132 {
            u32::from_le_bytes(raw[0x6C..0x70].try_into().unwrap_or([0; 4])) as u64
        } else {
            0
        };
        let size_bytes = (size_hi << 32) | size_lo;

        let flags = u32::from_le_bytes(raw[0x20..0x24].try_into().unwrap_or([0; 4]));
        let uses_extents = (flags & 0x0008_0000) != 0;

        let mut extents = Vec::new();
        if uses_extents {
            let i_block = &raw[0x28..0x28 + 60];
            let eh_magic = u16::from_le_bytes([i_block[0], i_block[1]]);
            if eh_magic == 0xF30A {
                let eh_entries = u16::from_le_bytes([i_block[2], i_block[3]]) as usize;
                let eh_depth = u16::from_le_bytes([i_block[6], i_block[7]]);

                // If leaf node (depth 0), parse direct extents embedded in inode
                if eh_depth == 0 {
                    let mut pos = 12usize;
                    for _ in 0..eh_entries.min(4) {
                        if pos + 12 <= i_block.len() {
                            let ee_block = u32::from_le_bytes(i_block[pos..pos + 4].try_into().unwrap_or([0; 4]));
                            let raw_len = u16::from_le_bytes([i_block[pos + 4], i_block[pos + 5]]);
                            let is_unwritten = (raw_len & 0x8000) != 0;
                            let block_count = raw_len & 0x7FFF;

                            let ee_start_hi = u16::from_le_bytes([i_block[pos + 6], i_block[pos + 7]]) as u64;
                            let ee_start_lo = u32::from_le_bytes(i_block[pos + 8..pos + 12].try_into().unwrap_or([0; 4])) as u64;
                            let physical_block = (ee_start_hi << 32) | ee_start_lo;

                            extents.push(Ext4Extent {
                                logical_block: ee_block,
                                block_count,
                                physical_block,
                                is_unwritten,
                            });
                            pos += 12;
                        }
                    }
                }
            }
        }

        Some(Self {
            inode_number,
            mode,
            size_bytes,
            is_directory,
            is_deleted,
            extents,
        })
    }
}

/// Parse linear EXT4 directory block entries (ext4_dir_entry_2)
pub fn parse_ext4_directory_block(block: &[u8]) -> Vec<Ext4DirEntry> {
    let mut entries = Vec::new();
    let mut offset = 0usize;

    while offset + 8 <= block.len() {
        let inode = u32::from_le_bytes(block[offset..offset + 4].try_into().unwrap_or([0; 4]));
        let rec_len = u16::from_le_bytes([block[offset + 4], block[offset + 5]]) as usize;
        let name_len = block[offset + 6] as usize;
        let file_type = block[offset + 7];

        if rec_len == 0 || offset + rec_len > block.len() {
            break;
        }

        if inode > 0 && name_len > 0 && offset + 8 + name_len <= offset + rec_len {
            let name_bytes = &block[offset + 8..offset + 8 + name_len];
            let name = String::from_utf8_lossy(name_bytes).to_string();
            entries.push(Ext4DirEntry {
                inode,
                file_type,
                name,
            });
        }

        offset += rec_len;
    }

    entries
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ext4_superblock_detection_and_parsing() {
        let mut buf = vec![0u8; 1024];
        // Inode count: 65536
        buf[0x00..0x04].copy_from_slice(&65536u32.to_le_bytes());
        // Block count: 262144
        buf[0x04..0x08].copy_from_slice(&262144u32.to_le_bytes());
        // Block size: 4096 (1024 << 2)
        buf[0x18..0x1C].copy_from_slice(&2u32.to_le_bytes());
        // Magic: 0xEF53
        buf[0x38..0x3A].copy_from_slice(&0xEF53u16.to_le_bytes());
        // Inode size: 256
        buf[0x58..0x5A].copy_from_slice(&256u16.to_le_bytes());
        // Volume name: "ROOT_FS"
        buf[0x78..0x7F].copy_from_slice(b"ROOT_FS");

        let sb = Ext4Superblock::parse(&buf).expect("Failed to parse valid EXT4 superblock");
        assert_eq!(sb.inodes_count, 65536);
        assert_eq!(sb.blocks_count, 262144);
        assert_eq!(sb.block_size, 4096);
        assert_eq!(sb.inode_size, 256);
        assert_eq!(sb.volume_name, "ROOT_FS");
    }

    #[test]
    fn test_ext4_extent_header_and_leaf_parsing() {
        let mut inode_raw = vec![0u8; 256];
        // Regular file mode: 0x81A4 (regular file, 0644)
        inode_raw[0x00..0x02].copy_from_slice(&0x81A4u16.to_le_bytes());
        // Size: 8192 bytes
        inode_raw[0x04..0x08].copy_from_slice(&8192u32.to_le_bytes());
        // Flags: EXT4_EXTENTS_FL (0x0008_0000)
        inode_raw[0x20..0x24].copy_from_slice(&0x0008_0000u32.to_le_bytes());

        // i_block extent header at 0x28
        let hdr_pos = 0x28;
        // eh_magic: 0xF30A
        inode_raw[hdr_pos..hdr_pos + 2].copy_from_slice(&0xF30Au16.to_le_bytes());
        // eh_entries: 1
        inode_raw[hdr_pos + 2..hdr_pos + 4].copy_from_slice(&1u16.to_le_bytes());
        // eh_max: 4
        inode_raw[hdr_pos + 4..hdr_pos + 6].copy_from_slice(&4u16.to_le_bytes());
        // eh_depth: 0 (leaf)
        inode_raw[hdr_pos + 6..hdr_pos + 8].copy_from_slice(&0u16.to_le_bytes());

        // Leaf extent 1 at hdr_pos + 12
        let ext_pos = hdr_pos + 12;
        // ee_block: 0 (starts at file block 0)
        inode_raw[ext_pos..ext_pos + 4].copy_from_slice(&0u32.to_le_bytes());
        // ee_len: 2 (2 blocks = 8192 bytes)
        inode_raw[ext_pos + 4..ext_pos + 6].copy_from_slice(&2u16.to_le_bytes());
        // ee_start: physical block 32768
        inode_raw[ext_pos + 6..ext_pos + 8].copy_from_slice(&0u16.to_le_bytes());
        inode_raw[ext_pos + 8..ext_pos + 12].copy_from_slice(&32768u32.to_le_bytes());

        let inode = Ext4Inode::parse(&inode_raw, 12).expect("Failed to parse EXT4 inode");
        assert_eq!(inode.size_bytes, 8192);
        assert!(!inode.is_directory);
        assert_eq!(inode.extents.len(), 1);
        assert_eq!(inode.extents[0].logical_block, 0);
        assert_eq!(inode.extents[0].block_count, 2);
        assert_eq!(inode.extents[0].physical_block, 32768);
    }
}
