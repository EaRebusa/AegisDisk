#![allow(dead_code)]
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crossbeam_channel::Sender;

use crate::common::error::Result;
use crate::core::recovery::carver::{CarvedFile, CarverEvent, PlaintextGate, SignatureCarver, SupportedFileType};
use crate::core::recovery::ext4::{Ext4Inode, Ext4Superblock, EXT4_SUPERBLOCK_OFFSET};
use crate::core::recovery::fat::{ExFatBootSector, Fat32BootSector, FatDirEntry};
use crate::core::recovery::metadata::{ClaimedLbaTracker, UndeleteEntry};
use crate::core::recovery::mft::{parse_mft_record, NtfsBootSector, MFT_RECORD_SIZE};
use crate::hal::buffer::AlignedBuffer;
use crate::hal::windows::open_direct_io_handle;

/// Configuration options for the Two-Tier Cascade Recovery Engine.
#[derive(Debug, Clone)]
pub struct CascadeConfig {
    pub output_dir: PathBuf,
    pub is_ssd: bool,
    pub plaintext_gate: PlaintextGate,
    pub sparse_enabled: bool,
    pub enable_metadata_pass: bool,
    pub enable_carver_pass: bool,
}

impl CascadeConfig {
    pub fn new(output_dir: PathBuf, is_ssd: bool) -> Self {
        Self {
            output_dir,
            is_ssd,
            plaintext_gate: PlaintextGate::Off,
            sparse_enabled: true,
            enable_metadata_pass: true,
            enable_carver_pass: true,
        }
    }
}

pub struct CascadeRecoveryEngine {
    pub config: CascadeConfig,
    pub claimed_tracker: Arc<ClaimedLbaTracker>,
}

impl CascadeRecoveryEngine {
    pub fn new(config: CascadeConfig) -> Self {
        Self {
            config,
            claimed_tracker: Arc::new(ClaimedLbaTracker::new()),
        }
    }

    /// Run cascade recovery on a physical drive using Direct I/O
    pub fn run_physical_drive(
        &self,
        disk_number: u32,
        total_bytes: u64,
        tx: Option<Sender<CarverEvent>>,
        pause: Option<Arc<AtomicBool>>,
        cancel: Option<Arc<AtomicBool>>,
    ) -> Result<Vec<CarvedFile>> {
        let mut all_recovered = Vec::new();

        if let Some(ref sender) = tx {
            let _ = sender.send(CarverEvent::Started { total_bytes });
        }

        let intact_dir = self.config.output_dir.join("intact_metadata");
        let raw_dir = self.config.output_dir.join("raw_carved");

        // Pass 1: Filesystem Metadata Scan (NTFS / FAT32 / exFAT / EXT4)
        if self.config.enable_metadata_pass {
            tracing::info!("Starting Tier 1 Metadata Undelete pass on PhysicalDrive{}", disk_number);
            let metadata_files = self.scan_metadata_physical(disk_number, total_bytes, &intact_dir, &tx, &cancel)?;
            for mf in metadata_files {
                self.claimed_tracker.claim_entry(&mf);
                let carved_repr = CarvedFile {
                    file_type: SupportedFileType::from_extension(
                        Path::new(&mf.name)
                            .extension()
                            .and_then(|s| s.to_str())
                            .unwrap_or(""),
                    )
                    .unwrap_or(SupportedFileType::Exe),
                    offset: mf.lba_ranges.first().map(|&(lba, _)| lba * 512).unwrap_or(0),
                    size_bytes: mf.file_size as usize,
                    output_path: intact_dir.join(&mf.name),
                };
                if let Some(ref sender) = tx {
                    let _ = sender.send(CarverEvent::FileFound(carved_repr.clone()));
                }
                all_recovered.push(carved_repr);
            }
        }

        // Pass 2: Raw Signature Carver (with Claimed LBA deduplication filter)
        if self.config.enable_carver_pass {
            if let Some(ref c) = cancel {
                if c.load(Ordering::Relaxed) {
                    return Ok(all_recovered);
                }
            }

            tracing::info!("Starting Tier 2 Raw Signature Carver pass with deduplication filter");
            let carver = SignatureCarver::new_with_options(
                raw_dir,
                self.config.is_ssd,
                self.config.plaintext_gate,
            )
            .with_claimed_tracker(Arc::clone(&self.claimed_tracker))
            .with_sparse(self.config.sparse_enabled);

            let carved_raw = carver.carve_physical_drive(
                disk_number,
                0,
                total_bytes,
                tx.clone(),
                pause,
                cancel,
            )?;
            all_recovered.extend(carved_raw);
        }

        if let Some(ref sender) = tx {
            let _ = sender.send(CarverEvent::Completed {
                total_found: all_recovered.len(),
            });
        }

        Ok(all_recovered)
    }

    /// Run cascade recovery on a raw image file (.raw / .img)
    pub fn run_from_file<P: AsRef<Path>>(
        &self,
        path: P,
        tx: Option<Sender<CarverEvent>>,
        pause: Option<Arc<AtomicBool>>,
        cancel: Option<Arc<AtomicBool>>,
    ) -> Result<Vec<CarvedFile>> {
        let mut all_recovered = Vec::new();
        let path_ref = path.as_ref();
        let mut file = File::open(path_ref)?;
        let total_bytes = file.metadata()?.len();

        if let Some(ref sender) = tx {
            let _ = sender.send(CarverEvent::Started { total_bytes });
        }

        let intact_dir = self.config.output_dir.join("intact_metadata");
        let raw_dir = self.config.output_dir.join("raw_carved");

        // Pass 1: Filesystem Metadata Scan on Image
        if self.config.enable_metadata_pass {
            tracing::info!("Starting Tier 1 Metadata Undelete pass on image {}", path_ref.display());
            let metadata_files = self.scan_metadata_file(&mut file, total_bytes, &intact_dir, &tx, &cancel)?;
            for mf in metadata_files {
                self.claimed_tracker.claim_entry(&mf);
                let carved_repr = CarvedFile {
                    file_type: SupportedFileType::from_extension(
                        Path::new(&mf.name)
                            .extension()
                            .and_then(|s| s.to_str())
                            .unwrap_or(""),
                    )
                    .unwrap_or(SupportedFileType::Exe),
                    offset: mf.lba_ranges.first().map(|&(lba, _)| lba * 512).unwrap_or(0),
                    size_bytes: mf.file_size as usize,
                    output_path: intact_dir.join(&mf.name),
                };
                if let Some(ref sender) = tx {
                    let _ = sender.send(CarverEvent::FileFound(carved_repr.clone()));
                }
                all_recovered.push(carved_repr);
            }
        }

        // Pass 2: Raw Signature Carver on Image
        if self.config.enable_carver_pass {
            if let Some(ref c) = cancel {
                if c.load(Ordering::Relaxed) {
                    return Ok(all_recovered);
                }
            }

            tracing::info!("Starting Tier 2 Raw Signature Carver pass on image");
            let carver = SignatureCarver::new_with_options(
                raw_dir,
                self.config.is_ssd,
                self.config.plaintext_gate,
            )
            .with_claimed_tracker(Arc::clone(&self.claimed_tracker))
            .with_sparse(self.config.sparse_enabled);

            let carved_raw = carver.carve_from_file(
                path_ref,
                tx.clone(),
                pause,
                cancel,
            )?;
            all_recovered.extend(carved_raw);
        }

        if let Some(ref sender) = tx {
            let _ = sender.send(CarverEvent::Completed {
                total_found: all_recovered.len(),
            });
        }

        Ok(all_recovered)
    }

    fn scan_metadata_physical(
        &self,
        disk_number: u32,
        total_bytes: u64,
        out_dir: &Path,
        _tx: &Option<Sender<CarverEvent>>,
        _cancel: &Option<Arc<AtomicBool>>,
    ) -> Result<Vec<UndeleteEntry>> {
        let handle_guard = open_direct_io_handle(disk_number, false)?;
        let handle = handle_guard.0;
        let mut sector_buf = AlignedBuffer::new(4096, 4096);

        if SignatureCarver::direct_read_chunk(handle, 0, 4096, &mut sector_buf).is_err() {
            return Ok(Vec::new());
        }

        let boot_slice = sector_buf.as_slice();
        fs::create_dir_all(out_dir)?;

        let mut results = Vec::new();

        // 1. Check for NTFS
        if let Some(ntfs) = NtfsBootSector::parse(boot_slice) {
            let mft_start_offset = ntfs.mft_start_lba() * ntfs.bytes_per_sector as u64;
            let mut record_buf = AlignedBuffer::new(MFT_RECORD_SIZE * 64, 4096);

            // Read first cluster of MFT records
            if mft_start_offset < total_bytes
                && SignatureCarver::direct_read_chunk(handle, mft_start_offset, record_buf.len(), &mut record_buf).is_ok()
            {
                let data = record_buf.as_slice();
                for (idx, chunk) in data.chunks_exact(MFT_RECORD_SIZE).enumerate() {
                    if let Some(deleted) = parse_mft_record(chunk, idx as u64) {
                        let entry = deleted.to_undelete_entry(ntfs.sectors_per_cluster as u64);
                        self.save_undelete_entry_direct(handle, &entry, out_dir, ntfs.bytes_per_sector as u64)?;
                        results.push(entry);
                    }
                }
            }
        }
        // 2. Check for FAT32
        else if let Some(fat32) = Fat32BootSector::parse(boot_slice) {
            if let Some(root_lba) = fat32.cluster_to_lba(fat32.root_cluster) {
                let root_offset = root_lba * fat32.bytes_per_sector as u64;
                let mut dir_buf = AlignedBuffer::new(4096, 4096);
                if root_offset < total_bytes
                    && SignatureCarver::direct_read_chunk(handle, root_offset, 4096, &mut dir_buf).is_ok()
                {
                    for (idx, chunk) in dir_buf.as_slice().chunks_exact(32).enumerate() {
                        if let Some(entry) = FatDirEntry::parse(chunk) {
                            if let Some(undel) = entry.to_undelete_entry(&fat32, idx as u64) {
                                self.save_undelete_entry_direct(handle, &undel, out_dir, fat32.bytes_per_sector as u64)?;
                                results.push(undel);
                            }
                        }
                    }
                }
            }
        }
        // 3. Check for exFAT
        else if let Some(exfat) = ExFatBootSector::parse(boot_slice) {
            if let Some(root_lba) = exfat.cluster_to_lba(exfat.root_dir_cluster) {
                let root_offset = root_lba * exfat.bytes_per_sector();
                let mut dir_buf = AlignedBuffer::new(4096, 4096);
                if root_offset < total_bytes
                    && SignatureCarver::direct_read_chunk(handle, root_offset, 4096, &mut dir_buf).is_ok()
                {
                    // Scan exFAT directory block for deleted files
                    for (idx, chunk) in dir_buf.as_slice().chunks_exact(32).enumerate() {
                        if chunk[0] == 0x05 { // Inactive/deleted file record
                            let mut undel = UndeleteEntry::new(idx as u64, format!("deleted_exfat_{:04}.bin", idx), PathBuf::new(), 4096, false);
                            undel.add_lba_range(root_lba + idx as u64, 8);
                            self.save_undelete_entry_direct(handle, &undel, out_dir, exfat.bytes_per_sector())?;
                            results.push(undel);
                        }
                    }
                }
            }
        }

        Ok(results)
    }

    fn scan_metadata_file(
        &self,
        file: &mut File,
        total_bytes: u64,
        out_dir: &Path,
        _tx: &Option<Sender<CarverEvent>>,
        _cancel: &Option<Arc<AtomicBool>>,
    ) -> Result<Vec<UndeleteEntry>> {
        fs::create_dir_all(out_dir)?;
        let mut boot_buf = vec![0u8; 4096];
        file.seek(SeekFrom::Start(0))?;
        let bytes_read = file.read(&mut boot_buf)?;
        if bytes_read < 512 {
            return Ok(Vec::new());
        }

        let mut results = Vec::new();

        // 1. Check NTFS
        if let Some(ntfs) = NtfsBootSector::parse(&boot_buf) {
            let mft_start_offset = ntfs.mft_start_lba() * ntfs.bytes_per_sector as u64;
            if mft_start_offset < total_bytes && file.seek(SeekFrom::Start(mft_start_offset)).is_ok() {
                let mut mft_buf = vec![0u8; MFT_RECORD_SIZE * 64];
                if let Ok(n) = file.read(&mut mft_buf) {
                    for (idx, chunk) in mft_buf[..n].chunks_exact(MFT_RECORD_SIZE).enumerate() {
                        if let Some(deleted) = parse_mft_record(chunk, idx as u64) {
                            let entry = deleted.to_undelete_entry(ntfs.sectors_per_cluster as u64);
                            self.save_undelete_entry_file(file, &entry, out_dir, ntfs.bytes_per_sector as u64)?;
                            results.push(entry);
                        }
                    }
                }
            }
        }
        // 2. Check FAT32
        else if let Some(fat32) = Fat32BootSector::parse(&boot_buf) {
            if let Some(root_lba) = fat32.cluster_to_lba(fat32.root_cluster) {
                let root_offset = root_lba * fat32.bytes_per_sector as u64;
                if root_offset < total_bytes && file.seek(SeekFrom::Start(root_offset)).is_ok() {
                    let mut dir_buf = vec![0u8; 4096];
                    if let Ok(n) = file.read(&mut dir_buf) {
                        for (idx, chunk) in dir_buf[..n].chunks_exact(32).enumerate() {
                            if let Some(entry) = FatDirEntry::parse(chunk) {
                                if let Some(undel) = entry.to_undelete_entry(&fat32, idx as u64) {
                                    self.save_undelete_entry_file(file, &undel, out_dir, fat32.bytes_per_sector as u64)?;
                                    results.push(undel);
                                }
                            }
                        }
                    }
                }
            }
        }
        // 3. Check EXT4 (Superblock at offset 1024)
        else if total_bytes > EXT4_SUPERBLOCK_OFFSET + 1024 {
            file.seek(SeekFrom::Start(EXT4_SUPERBLOCK_OFFSET))?;
            let mut sb_buf = vec![0u8; 1024];
            if file.read_exact(&mut sb_buf).is_ok() {
                if let Some(sb) = Ext4Superblock::parse(&sb_buf) {
                    let sectors_per_block = (sb.block_size / 512) as u64;
                    // Scan first inode table block
                    let inode_table_block = (sb.first_data_block + 1) as u64;
                    let table_offset = inode_table_block * sb.block_size as u64;
                    if table_offset < total_bytes && file.seek(SeekFrom::Start(table_offset)).is_ok() {
                        let mut inode_buf = vec![0u8; sb.inode_size as usize * 16];
                        if let Ok(n) = file.read(&mut inode_buf) {
                            for (idx, chunk) in inode_buf[..n].chunks_exact(sb.inode_size as usize).enumerate() {
                                if let Some(inode) = Ext4Inode::parse(chunk, (idx + 1) as u32) {
                                    if inode.is_deleted && !inode.is_directory && inode.size_bytes > 0 {
                                        let undel = inode.to_undelete_entry(sectors_per_block, format!("deleted_ext4_inode_{}", inode.inode_number));
                                        self.save_undelete_entry_file(file, &undel, out_dir, 512)?;
                                        results.push(undel);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        Ok(results)
    }

    fn save_undelete_entry_direct(
        &self,
        handle: windows_sys::Win32::Foundation::HANDLE,
        entry: &UndeleteEntry,
        out_dir: &Path,
        bytes_per_sector: u64,
    ) -> Result<()> {
        let out_file_path = out_dir.join(&entry.name);
        let mut out = File::create(&out_file_path)?;

        if self.config.sparse_enabled {
            crate::core::imager::cloner::mark_file_sparse(&out);
        }

        if let Some(ref res_data) = entry.resident_data {
            out.write_all(res_data)?;
            return Ok(());
        }

        let mut remaining = entry.file_size;
        let mut scratch = AlignedBuffer::new(64 * 1024, 4096);

        for &(start_lba, sector_count) in &entry.lba_ranges {
            if remaining == 0 {
                break;
            }
            let extent_bytes = sector_count * bytes_per_sector;
            let to_read_total = extent_bytes.min(remaining);
            let mut extent_offset = start_lba * bytes_per_sector;
            let mut extent_remaining = to_read_total;

            while extent_remaining > 0 {
                let chunk_size = (extent_remaining as usize).min(scratch.len());
                let aligned_chunk = ((chunk_size + 4095) / 4096) * 4096;
                let n = match SignatureCarver::direct_read_chunk(handle, extent_offset, aligned_chunk, &mut scratch) {
                    Ok(read_bytes) => (read_bytes as usize).min(chunk_size),
                    Err(_) => break,
                };
                if n == 0 {
                    break;
                }
                out.write_all(&scratch.as_slice()[..n])?;
                extent_offset += n as u64;
                extent_remaining -= n as u64;
                remaining = remaining.saturating_sub(n as u64);
            }
        }

        Ok(())
    }

    fn save_undelete_entry_file(
        &self,
        source: &mut File,
        entry: &UndeleteEntry,
        out_dir: &Path,
        bytes_per_sector: u64,
    ) -> Result<()> {
        let out_file_path = out_dir.join(&entry.name);
        let mut out = File::create(&out_file_path)?;

        if self.config.sparse_enabled {
            crate::core::imager::cloner::mark_file_sparse(&out);
        }

        if let Some(ref res_data) = entry.resident_data {
            out.write_all(res_data)?;
            return Ok(());
        }

        let mut remaining = entry.file_size;
        let mut buffer = vec![0u8; 64 * 1024];

        for &(start_lba, sector_count) in &entry.lba_ranges {
            if remaining == 0 {
                break;
            }
            let extent_bytes = sector_count * bytes_per_sector;
            let to_read_total = extent_bytes.min(remaining);
            let mut extent_offset = start_lba * bytes_per_sector;
            let mut extent_remaining = to_read_total;

            while extent_remaining > 0 {
                let chunk_size = (extent_remaining as usize).min(buffer.len());
                source.seek(SeekFrom::Start(extent_offset))?;
                let n = source.read(&mut buffer[..chunk_size])?;
                if n == 0 {
                    break;
                }
                out.write_all(&buffer[..n])?;
                extent_offset += n as u64;
                extent_remaining -= n as u64;
                remaining = remaining.saturating_sub(n as u64);
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cascade_recovery_dual_pass_and_dedup() {
        let temp_dir = std::env::temp_dir().join(format!("cascade_test_{}", std::process::id()));
        let _ = fs::create_dir_all(&temp_dir);
        let image_path = temp_dir.join("test_disk.raw");

        // Create a 1MB test disk image
        let disk_size = 1024 * 1024;
        let mut disk_data = vec![0u8; disk_size];

        // 1. Write FAT32 VBR at sector 0
        disk_data[510] = 0x55;
        disk_data[511] = 0xAA;
        disk_data[0x0B..0x0D].copy_from_slice(&512u16.to_le_bytes()); // 512 B/sector
        disk_data[0x0D] = 8; // 8 sectors/cluster (4096 B)
        disk_data[0x0E..0x10].copy_from_slice(&32u16.to_le_bytes()); // 32 reserved sectors
        disk_data[0x10] = 2; // 2 FATs
        disk_data[0x24..0x28].copy_from_slice(&32u32.to_le_bytes()); // 32 sectors per FAT
        disk_data[0x2C..0x30].copy_from_slice(&2u32.to_le_bytes()); // root cluster 2

        // Root dir starts at LBA: 32 + (2 * 32) = 96 (offset 49152)
        let root_dir_offset = 96 * 512;
        // Deleted directory entry 1: "APP.EXE", start cluster 3 (LBA 96 + 8 = 104, offset 53248), size 4096 bytes
        let entry1_offset = root_dir_offset;
        disk_data[entry1_offset] = 0xE5; // Deleted tombstone
        disk_data[entry1_offset + 1..entry1_offset + 8].copy_from_slice(b"PP     ");
        disk_data[entry1_offset + 8..entry1_offset + 11].copy_from_slice(b"EXE");
        disk_data[entry1_offset + 0x0B] = 0x20;
        disk_data[entry1_offset + 0x1A..entry1_offset + 0x1C].copy_from_slice(&3u16.to_le_bytes()); // Cluster 3
        disk_data[entry1_offset + 0x1C..entry1_offset + 0x20].copy_from_slice(&4096u32.to_le_bytes()); // 4096 bytes

        // Fill Cluster 3 with simulated executable content
        let cluster3_offset = 104 * 512;
        disk_data[cluster3_offset..cluster3_offset + 2].copy_from_slice(b"MZ");
        disk_data[cluster3_offset + 2..cluster3_offset + 4096].fill(0x90);

        // 2. Put an orphan raw BMP at offset 120 * 512 (LBA 120, not in FAT directory table)
        let bmp_offset = 120 * 512;
        disk_data[bmp_offset..bmp_offset + 2].copy_from_slice(b"BM");
        disk_data[bmp_offset + 2..bmp_offset + 6].copy_from_slice(&4096u32.to_le_bytes());
        disk_data[bmp_offset + 10..bmp_offset + 14].copy_from_slice(&54u32.to_le_bytes());
        disk_data[bmp_offset + 14..bmp_offset + 18].copy_from_slice(&40u32.to_le_bytes());
        disk_data[bmp_offset + 18..bmp_offset + 22].copy_from_slice(&32i32.to_le_bytes());
        disk_data[bmp_offset + 22..bmp_offset + 26].copy_from_slice(&32i32.to_le_bytes());
        disk_data[bmp_offset + 26..bmp_offset + 28].copy_from_slice(&1u16.to_le_bytes());
        disk_data[bmp_offset + 28..bmp_offset + 30].copy_from_slice(&24u16.to_le_bytes());

        let mut f = File::create(&image_path).unwrap();
        f.write_all(&disk_data).unwrap();
        drop(f);

        let out_dir = temp_dir.join("recovered");
        let mut config = CascadeConfig::new(out_dir.clone(), false);
        config.sparse_enabled = false;
        let engine = CascadeRecoveryEngine::new(config);

        let recovered = engine.run_from_file(&image_path, None, None, None).unwrap();
        assert_eq!(recovered.len(), 2, "Expected 1 metadata recovered file and 1 raw carved file");

        // Pass 1 file must be in intact_metadata
        let intact_file = out_dir.join("intact_metadata").join("_PP.EXE");
        assert!(intact_file.exists(), "Pass 1 intact file must exist");
        assert_eq!(fs::metadata(&intact_file).unwrap().len(), 4096);

        // Pass 2 file must be in raw_carved
        let raw_entries: Vec<_> = fs::read_dir(out_dir.join("raw_carved")).unwrap().collect();
        assert_eq!(raw_entries.len(), 1, "Only 1 orphan BMP carved");

        // Claimed tracker must contain the sectors of cluster 3 (LBAs 104..112)
        assert!(engine.claimed_tracker.is_claimed(104));
        assert!(engine.claimed_tracker.is_claimed(111));
        assert!(!engine.claimed_tracker.is_claimed(120), "BMP sector was not in metadata");

        let _ = fs::remove_dir_all(&temp_dir);
    }
}
