#![allow(dead_code)]
use std::path::PathBuf;
use crate::core::recovery::metadata::UndeleteEntry;

/// FAT32 Volume Boot Record (VBR) header
#[derive(Debug, Clone)]
pub struct Fat32BootSector {
    pub bytes_per_sector: u16,
    pub sectors_per_cluster: u8,
    pub reserved_sectors: u16,
    pub fat_count: u8,
    pub sectors_per_fat: u32,
    pub root_cluster: u32,
    pub total_sectors: u32,
}

impl Fat32BootSector {
    pub fn parse(raw: &[u8]) -> Option<Self> {
        if raw.len() < 512 {
            return None;
        }

        // Check boot signature 0x55AA at offset 510
        if raw[510] != 0x55 || raw[511] != 0xAA {
            return None;
        }

        let bytes_per_sector = u16::from_le_bytes([raw[0x0B], raw[0x0C]]);
        let sectors_per_cluster = raw[0x0D];
        let reserved_sectors = u16::from_le_bytes([raw[0x0E], raw[0x0F]]);
        let fat_count = raw[0x10];

        if bytes_per_sector == 0 || sectors_per_cluster == 0 || fat_count == 0 {
            return None;
        }

        let sectors_per_fat_16 = u16::from_le_bytes([raw[0x16], raw[0x17]]) as u32;
        let sectors_per_fat = if sectors_per_fat_16 == 0 {
            u32::from_le_bytes([raw[0x24], raw[0x25], raw[0x26], raw[0x27]])
        } else {
            sectors_per_fat_16
        };

        if sectors_per_fat == 0 {
            return None;
        }

        let root_cluster = u32::from_le_bytes([raw[0x2C], raw[0x2D], raw[0x2E], raw[0x2F]]);
        let total_sectors_16 = u16::from_le_bytes([raw[0x13], raw[0x14]]) as u32;
        let total_sectors = if total_sectors_16 == 0 {
            u32::from_le_bytes([raw[0x20], raw[0x21], raw[0x22], raw[0x23]])
        } else {
            total_sectors_16
        };

        Some(Self {
            bytes_per_sector,
            sectors_per_cluster,
            reserved_sectors,
            fat_count,
            sectors_per_fat,
            root_cluster,
            total_sectors,
        })
    }

    /// Calculate physical start LBA of the cluster data area
    pub fn data_region_lba(&self) -> u64 {
        self.reserved_sectors as u64 + (self.fat_count as u64 * self.sectors_per_fat as u64)
    }

    /// Convert cluster number into absolute LBA sector offset
    pub fn cluster_to_lba(&self, cluster: u32) -> Option<u64> {
        if cluster < 2 {
            return None;
        }
        let cluster_offset = (cluster - 2) as u64 * self.sectors_per_cluster as u64;
        Some(self.data_region_lba() + cluster_offset)
    }
}

/// Standard 32-byte FAT directory entry
#[derive(Debug, Clone)]
pub struct FatDirEntry {
    pub name: String,
    pub is_deleted: bool,
    pub is_directory: bool,
    pub start_cluster: u32,
    pub file_size: u32,
}

impl FatDirEntry {
    pub fn parse(raw: &[u8]) -> Option<Self> {
        if raw.len() < 32 {
            return None;
        }

        let first_byte = raw[0];
        if first_byte == 0x00 {
            // End of directory marker
            return None;
        }

        // 0xE5 indicates a deleted entry (tombstone)
        let is_deleted = first_byte == 0xE5;

        // Attribute byte at offset 0x0B
        let attr = raw[0x0B];
        if attr == 0x0F {
            // Long filename (LFN) entry; skipped for basic raw entry parsing
            return None;
        }

        let is_directory = (attr & 0x10) != 0;

        // Short 8.3 filename parsing
        let mut name_part = String::new();
        if is_deleted {
            name_part.push('_');
            name_part.push_str(&String::from_utf8_lossy(&raw[1..8]).trim());
        } else {
            name_part.push_str(&String::from_utf8_lossy(&raw[0..8]).trim());
        }

        let ext_part = String::from_utf8_lossy(&raw[8..11]).trim().to_string();
        let filename = if ext_part.is_empty() || is_directory {
            name_part
        } else {
            format!("{}.{}", name_part, ext_part)
        };

        let cluster_hi = u16::from_le_bytes([raw[0x14], raw[0x15]]) as u32;
        let cluster_lo = u16::from_le_bytes([raw[0x1A], raw[0x1B]]) as u32;
        let start_cluster = (cluster_hi << 16) | cluster_lo;

        let file_size = u32::from_le_bytes([raw[0x1C], raw[0x1D], raw[0x1E], raw[0x1F]]);

        Some(Self {
            name: filename,
            is_deleted,
            is_directory,
            start_cluster,
            file_size,
        })
    }

    /// Convert deleted FAT entry to a unified UndeleteEntry
    pub fn to_undelete_entry(&self, boot: &Fat32BootSector, entry_idx: u64) -> Option<UndeleteEntry> {
        if !self.is_deleted || self.start_cluster < 2 {
            return None;
        }

        let start_lba = boot.cluster_to_lba(self.start_cluster)?;
        let cluster_bytes = (boot.sectors_per_cluster as u64) * (boot.bytes_per_sector as u64);
        if cluster_bytes == 0 {
            return None;
        }

        let clusters_needed = (self.file_size as u64).div_ceil(cluster_bytes).max(1);
        let total_sectors = clusters_needed * boot.sectors_per_cluster as u64;

        let mut entry = UndeleteEntry::new(
            entry_idx,
            self.name.clone(),
            PathBuf::new(),
            self.file_size as u64,
            self.is_directory,
        );
        entry.add_lba_range(start_lba, total_sectors);
        Some(entry)
    }
}

/// exFAT Volume Boot Record
#[derive(Debug, Clone)]
pub struct ExFatBootSector {
    pub bytes_per_sector_shift: u8,
    pub sectors_per_cluster_shift: u8,
    pub cluster_heap_offset: u32,
    pub cluster_count: u32,
    pub root_dir_cluster: u32,
}

impl ExFatBootSector {
    pub fn parse(raw: &[u8]) -> Option<Self> {
        if raw.len() < 512 || &raw[3..11] != b"EXFAT   " {
            return None;
        }

        let bytes_per_sector_shift = raw[0x6C];
        let sectors_per_cluster_shift = raw[0x6D];
        if bytes_per_sector_shift < 9 || bytes_per_sector_shift > 12 {
            return None;
        }

        let cluster_heap_offset = u32::from_le_bytes([raw[0x58], raw[0x59], raw[0x5A], raw[0x5B]]);
        let cluster_count = u32::from_le_bytes([raw[0x5C], raw[0x5D], raw[0x5E], raw[0x5F]]);
        let root_dir_cluster = u32::from_le_bytes([raw[0x60], raw[0x61], raw[0x62], raw[0x63]]);

        Some(Self {
            bytes_per_sector_shift,
            sectors_per_cluster_shift,
            cluster_heap_offset,
            cluster_count,
            root_dir_cluster,
        })
    }

    pub fn bytes_per_sector(&self) -> u64 {
        1u64 << self.bytes_per_sector_shift
    }

    pub fn sectors_per_cluster(&self) -> u64 {
        1u64 << self.sectors_per_cluster_shift
    }

    pub fn cluster_to_lba(&self, cluster: u32) -> Option<u64> {
        if cluster < 2 {
            return None;
        }
        let cluster_idx = (cluster - 2) as u64;
        Some(self.cluster_heap_offset as u64 + cluster_idx * self.sectors_per_cluster())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fat32_boot_sector_and_cluster_mapping() {
        let mut vbr = vec![0u8; 512];
        vbr[510] = 0x55;
        vbr[511] = 0xAA;
        vbr[0x0B..0x0D].copy_from_slice(&512u16.to_le_bytes()); // 512 bytes per sector
        vbr[0x0D] = 8; // 8 sectors per cluster (4KB)
        vbr[0x0E..0x10].copy_from_slice(&32u16.to_le_bytes()); // 32 reserved sectors
        vbr[0x10] = 2; // 2 FATs
        vbr[0x24..0x28].copy_from_slice(&1000u32.to_le_bytes()); // 1000 sectors per FAT
        vbr[0x2C..0x30].copy_from_slice(&2u32.to_le_bytes()); // root cluster 2

        let boot = Fat32BootSector::parse(&vbr).expect("Failed to parse valid FAT32 VBR");
        assert_eq!(boot.bytes_per_sector, 512);
        assert_eq!(boot.sectors_per_cluster, 8);
        assert_eq!(boot.data_region_lba(), 32 + (2 * 1000)); // 2032
        assert_eq!(boot.cluster_to_lba(2), Some(2032));
        assert_eq!(boot.cluster_to_lba(3), Some(2040));
    }

    #[test]
    fn test_fat32_deleted_entry_undelete() {
        let mut vbr = vec![0u8; 512];
        vbr[510] = 0x55;
        vbr[511] = 0xAA;
        vbr[0x0B..0x0D].copy_from_slice(&512u16.to_le_bytes());
        vbr[0x0D] = 8;
        vbr[0x0E..0x10].copy_from_slice(&32u16.to_le_bytes());
        vbr[0x10] = 2;
        vbr[0x24..0x28].copy_from_slice(&1000u32.to_le_bytes());
        let boot = Fat32BootSector::parse(&vbr).unwrap();

        let mut entry_raw = vec![0u8; 32];
        entry_raw[0] = 0xE5; // Deleted
        entry_raw[1..8].copy_from_slice(b"ESTFILE");
        entry_raw[8..11].copy_from_slice(b"EXE");
        entry_raw[0x0B] = 0x20; // Archive attribute
        entry_raw[0x14..0x16].copy_from_slice(&0u16.to_le_bytes());
        entry_raw[0x1A..0x1C].copy_from_slice(&10u16.to_le_bytes()); // Cluster 10
        entry_raw[0x1C..0x20].copy_from_slice(&4096u32.to_le_bytes()); // 4096 bytes

        let parsed = FatDirEntry::parse(&entry_raw).expect("Failed to parse FAT deleted entry");
        assert!(parsed.is_deleted);
        assert_eq!(parsed.name, "_ESTFILE.EXE");

        let undelete = parsed.to_undelete_entry(&boot, 1).expect("Failed to create undelete entry");
        assert_eq!(undelete.file_size, 4096);
        assert_eq!(undelete.lba_ranges.len(), 1);
        let expected_lba = 2032 + (8 * 8); // 2096
        assert_eq!(undelete.lba_ranges[0], (expected_lba, 8));
    }
}
