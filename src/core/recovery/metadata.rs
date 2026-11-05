#![allow(dead_code)]
use std::path::PathBuf;
use std::sync::RwLock;
use roaring::RoaringTreemap;
use crate::common::error::Result;

/// Unified descriptor for a deleted or active file discovered via filesystem metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UndeleteEntry {
    /// Internal record identifier (MFT record number, Inode, or FAT cluster).
    pub record_id: u64,
    /// File or directory base name.
    pub name: String,
    /// Relative directory path if parent hierarchy can be resolved.
    pub parent_path: PathBuf,
    /// Total logical file size in bytes.
    pub file_size: u64,
    /// True if entry represents a directory.
    pub is_directory: bool,
    /// True if the entry was flagged as deleted / inactive in filesystem metadata.
    pub is_deleted: bool,
    /// List of physical disk LBA extents: (start_lba, sector_count).
    pub lba_ranges: Vec<(u64, u64)>,
    /// Optional payload for resident files (e.g., small NTFS files embedded directly in $MFT record).
    pub resident_data: Option<Vec<u8>>,
}

impl UndeleteEntry {
    /// Construct a new deleted file record.
    pub fn new(
        record_id: u64,
        name: String,
        parent_path: PathBuf,
        file_size: u64,
        is_directory: bool,
    ) -> Self {
        Self {
            record_id,
            name,
            parent_path,
            file_size,
            is_directory,
            is_deleted: true,
            lba_ranges: Vec::new(),
            resident_data: None,
        }
    }

    /// Add an LBA extent to this file record.
    pub fn add_lba_range(&mut self, start_lba: u64, sector_count: u64) {
        if sector_count > 0 {
            self.lba_ranges.push((start_lba, sector_count));
        }
    }

    /// Calculate total sectors spanned by all data extents.
    pub fn total_sectors(&self) -> u64 {
        self.lba_ranges.iter().map(|(_, count)| *count).sum()
    }
}

/// Thread-safe tracker mapping allocated and claimed physical LBA sectors.
#[derive(Debug, Default)]
pub struct ClaimedLbaTracker {
    bitmap: RwLock<RoaringTreemap>,
}

impl ClaimedLbaTracker {
    pub fn new() -> Self {
        Self {
            bitmap: RwLock::new(RoaringTreemap::new()),
        }
    }

    /// Mark an LBA range as claimed by a valid recovered metadata entry.
    pub fn claim_range(&self, start_lba: u64, sector_count: u64) {
        if sector_count == 0 {
            return;
        }
        let end_lba = start_lba.saturating_add(sector_count);
        if let Ok(mut map) = self.bitmap.write() {
            map.insert_range(start_lba..end_lba);
        }
    }

    /// Mark all extents of an undelete candidate as claimed.
    pub fn claim_entry(&self, entry: &UndeleteEntry) {
        for &(start_lba, count) in &entry.lba_ranges {
            self.claim_range(start_lba, count);
        }
    }

    /// Check whether an LBA has already been claimed by Tier 1 metadata recovery.
    pub fn is_claimed(&self, lba: u64) -> bool {
        self.bitmap
            .read()
            .map(|map| map.contains(lba))
            .unwrap_or(false)
    }

    /// Check if any LBA in the designated sector range is claimed.
    pub fn any_claimed_in_range(&self, start_lba: u64, sector_count: u64) -> bool {
        if sector_count == 0 {
            return false;
        }
        let end_lba = start_lba.saturating_add(sector_count);
        let map = match self.bitmap.read() {
            Ok(m) => m,
            Err(_) => return false,
        };

        // Fast check on boundary points first
        if map.contains(start_lba) || (end_lba > start_lba && map.contains(end_lba - 1)) {
            return true;
        }

        // Iterate small ranges directly
        for lba in start_lba..end_lba {
            if map.contains(lba) {
                return true;
            }
        }
        false
    }

    /// Return total number of claimed sectors.
    pub fn total_claimed_sectors(&self) -> u64 {
        self.bitmap.read().map(|m| m.len()).unwrap_or(0)
    }

    /// Clear all claimed ranges.
    pub fn clear(&self) {
        if let Ok(mut map) = self.bitmap.write() {
            map.clear();
        }
    }
}

/// Abstract contract implemented by filesystem-specific undelete modules (NTFS, EXT4, FAT32, exFAT).
pub trait MetadataScanner: Send + Sync {
    /// Name or identifier of filesystem type (e.g. "NTFS", "EXT4").
    fn filesystem_name(&self) -> &'static str;

    /// Scan metadata records and return all recoverable deleted file candidates.
    fn scan_deleted_files(&mut self) -> Result<Vec<UndeleteEntry>>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_claim_single_and_range() {
        let tracker = ClaimedLbaTracker::new();
        assert!(!tracker.is_claimed(100));

        tracker.claim_range(100, 10);
        assert!(tracker.is_claimed(100));
        assert!(tracker.is_claimed(105));
        assert!(tracker.is_claimed(109));
        assert!(!tracker.is_claimed(110));
        assert_eq!(tracker.total_claimed_sectors(), 10);
    }

    #[test]
    fn test_claim_entry() {
        let tracker = ClaimedLbaTracker::new();
        let mut entry = UndeleteEntry::new(
            42,
            "kernel32.dll".to_string(),
            PathBuf::from("Windows/System32"),
            8192,
            false,
        );
        entry.add_lba_range(1000, 8);
        entry.add_lba_range(2000, 8);

        tracker.claim_entry(&entry);
        assert!(tracker.is_claimed(1000));
        assert!(tracker.is_claimed(1007));
        assert!(!tracker.is_claimed(1008));
        assert!(tracker.is_claimed(2000));
        assert_eq!(tracker.total_claimed_sectors(), 16);
    }

    #[test]
    fn test_any_claimed_in_range() {
        let tracker = ClaimedLbaTracker::new();
        tracker.claim_range(500, 50);

        assert!(tracker.any_claimed_in_range(490, 20)); // overlaps 500..510
        assert!(!tracker.any_claimed_in_range(400, 50)); // disjoint before
        assert!(!tracker.any_claimed_in_range(600, 50)); // disjoint after
    }
}
