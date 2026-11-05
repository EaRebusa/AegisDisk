use std::fs::{File, OpenOptions};
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::ptr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use crossbeam_channel::Sender;
use windows_sys::Win32::Foundation::{
    CloseHandle, GetLastError, ERROR_DEVICE_NOT_CONNECTED, ERROR_IO_PENDING, HANDLE,
    INVALID_HANDLE_VALUE, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, GetDiskFreeSpaceExW, ReadFile, FILE_FLAG_NO_BUFFERING, FILE_FLAG_OVERLAPPED,
    FILE_GENERIC_READ, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows_sys::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
use windows_sys::Win32::System::Threading::{CreateEventW, WaitForSingleObject};

use crate::common::error::{AegisError, Result};
use crate::common::types::DiskInfo;
use crate::core::imager::mapfile::{BlockStatus, RescueMap};
use crate::core::telemetry::poll_temperature;
use crate::hal::buffer::AlignedBuffer;
use crate::hal::windows::{open_direct_io_handle, query_filesystem_kind, to_wide_chars, FileSystemKind};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImagingMode {
    ForensicRaw,     // Full bit-by-bit raw DD with slack space preservation
    SmartFilesystem, // Smart NTFS $Bitmap mode: skips unallocated clusters entirely
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum ImagerEvent {
    Started { total_bytes: u64, mode: ImagingMode, sparse: bool },
    Progress {
        pos: u64,
        rescued_bytes: u64,
        bad_bytes: u64,
        pending_bytes: u64,
        speed_mb_s: f64,
        pass: u8,
        temp_c: Option<u32>,
    },
    ThermalPause { temp_c: u32, threshold_c: u32 },
    ThermalResumed { temp_c: u32 },
    DeviceDisconnected,
    DeviceReconnected,
    Completed { total_rescued: u64, total_bad: u64 },
    Failed(String),
}

#[derive(Debug, Clone)]
pub struct ImagerConfig {
    pub read_timeout_ms: u32,
    pub max_temp_c: u32,
    pub chunk_size: usize,
    pub skip_size_on_error: u64,
    pub mode: ImagingMode,
    pub sparse_destination: bool,
}

impl Default for ImagerConfig {
    fn default() -> Self {
        Self {
            read_timeout_ms: 500,
            max_temp_c: 55,
            chunk_size: 64 * 1024, // 64 KB
            skip_size_on_error: 1024 * 1024, // 1 MB
            mode: ImagingMode::ForensicRaw,
            sparse_destination: true,
        }
    }
}

pub fn preflight_destination_check(dest_path: &Path, required_bytes: u64, sparse_enabled: bool) -> Result<()> {
    let parent = if let Ok(abs) = std::fs::canonicalize(dest_path.parent().unwrap_or(Path::new("."))) {
        abs
    } else {
        std::env::current_dir().unwrap_or_else(|_| PathBuf::from("C:\\"))
    };
    let fs_kind = query_filesystem_kind(&parent);
    let dir_str = parent.to_string_lossy().to_string();
    let wide_dir = to_wide_chars(&dir_str);

    unsafe {
        let mut free_available = 0u64;
        let mut total_bytes = 0u64;
        let mut total_free = 0u64;

        let res = GetDiskFreeSpaceExW(
            wide_dir.as_ptr(),
            &mut free_available,
            &mut total_bytes,
            &mut total_free,
        );

        if res == 0 {
            let root_str = "C:\\\0".encode_utf16().collect::<Vec<u16>>();
            let root_res = GetDiskFreeSpaceExW(
                root_str.as_ptr(),
                &mut free_available,
                &mut total_bytes,
                &mut total_free,
            );
            if root_res == 0 {
                let err = GetLastError();
                return Err(AegisError::IoctlFailed("GetDiskFreeSpaceExW failed".to_string(), err));
            }
        }

        // When sparse is enabled on NTFS, enforce only a 5 GB safety reserve
        let effective_sparse = sparse_enabled && fs_kind.supports_sparse();
        let min_required = if effective_sparse {
            5 * 1024 * 1024 * 1024 // 5 GB safety margin
        } else {
            (required_bytes as f64 * 1.05) as u64 // +5% safety margin
        };

        if free_available < min_required {
            return Err(AegisError::SnapshotFailed(
                0,
                format!(
                    "Insufficient destination space: {:.2} GB free, required at least {:.2} GB ({})",
                    free_available as f64 / 1e9,
                    min_required as f64 / 1e9,
                    if !fs_kind.supports_sparse() && sparse_enabled {
                        format!("{} does not support sparse; requires full physical capacity", fs_kind.as_str())
                    } else {
                        "Required allocation".to_string()
                    }
                ),
            ));
        }
    }

    Ok(())
}

pub struct MultiPartImageWriter {
    base_path: PathBuf,
    is_fat32: bool,
    max_chunk_size: u64,
    current_chunk_idx: usize,
    current_file: Option<File>,
}

impl MultiPartImageWriter {
    pub fn new(base_path: PathBuf, is_fat32: bool, is_sparse: bool) -> Result<Self> {
        let mut writer = Self {
            base_path,
            is_fat32,
            max_chunk_size: 3_900_000_000u64, // 3.9 GB boundary for FAT32
            current_chunk_idx: 0,
            current_file: None,
        };
        writer.open_chunk(0, is_sparse)?;
        Ok(writer)
    }

    fn open_chunk(&mut self, idx: usize, is_sparse: bool) -> Result<()> {
        let path = if self.is_fat32 {
            let stem = self.base_path.file_stem().unwrap_or_default().to_string_lossy();
            let parent = self.base_path.parent().unwrap_or(Path::new("."));
            parent.join(format!("{}.raw.{:03}", stem, idx + 1))
        } else {
            self.base_path.clone()
        };

        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .open(&path)?;

        if is_sparse {
            mark_file_sparse(&file);
        }

        self.current_file = Some(file);
        self.current_chunk_idx = idx;
        Ok(())
    }

    pub fn write_at(&mut self, global_pos: u64, data: &[u8], is_sparse: bool) -> std::io::Result<()> {
        let chunk_idx = if self.is_fat32 {
            (global_pos / self.max_chunk_size) as usize
        } else {
            0
        };

        if chunk_idx != self.current_chunk_idx || self.current_file.is_none() {
            if let Some(mut old) = self.current_file.take() {
                let _ = old.flush();
            }
            let _ = self.open_chunk(chunk_idx, is_sparse);
        }

        if let Some(ref mut file) = self.current_file {
            let offset_in_chunk = if self.is_fat32 {
                global_pos % self.max_chunk_size
            } else {
                global_pos
            };
            file.seek(SeekFrom::Start(offset_in_chunk))?;
            file.write_all(data)?;
        }

        Ok(())
    }

    pub fn flush(&mut self) -> std::io::Result<()> {
        if let Some(ref mut file) = self.current_file {
            file.flush()?;
        }
        Ok(())
    }

    pub fn finalize(&mut self, total_bytes: u64) -> std::io::Result<()> {
        if let Some(ref mut file) = self.current_file {
            if !self.is_fat32 {
                let _ = file.set_len(total_bytes);
            }
            file.flush()?;
        }
        Ok(())
    }
}

/// Enable NTFS Sparse File allocation (FSCTL_SET_SPARSE) on destination image
pub fn mark_file_sparse(file: &File) -> bool {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::System::IO::DeviceIoControl;
    const FSCTL_SET_SPARSE: u32 = 0x000900C4;

    unsafe {
        let mut bytes_returned = 0u32;
        let res = DeviceIoControl(
            file.as_raw_handle() as HANDLE,
            FSCTL_SET_SPARSE,
            ptr::null(),
            0,
            ptr::null_mut(),
            0,
            &mut bytes_returned,
            ptr::null_mut(),
        );
        res != 0
    }
}

/// Fast vectorized check if a buffer contains only zeroes
#[inline]
pub fn is_all_zero(slice: &[u8]) -> bool {
    let (prefix, chunks, suffix) = unsafe { slice.align_to::<u64>() };
    prefix.iter().all(|&b| b == 0)
        && chunks.iter().all(|&w| w == 0)
        && suffix.iter().all(|&b| b == 0)
}

/// Open an Overlapped Direct I/O handle for bad-sector reading
fn open_overlapped_read_handle(disk_number: u32) -> Result<HANDLE> {
    let path = format!("\\\\.\\PhysicalDrive{}", disk_number);
    let wide_path = to_wide_chars(&path);

    unsafe {
        let handle = CreateFileW(
            wide_path.as_ptr(),
            FILE_GENERIC_READ,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_NO_BUFFERING | FILE_FLAG_OVERLAPPED,
            ptr::null_mut(),
        );

        if handle == INVALID_HANDLE_VALUE {
            let err = GetLastError();
            return Err(AegisError::DeviceOpenFailed(path, err));
        }

        Ok(handle)
    }
}

/// Read block with Overlapped I/O and strict CancelIoEx watchdog
fn read_block_with_watchdog(
    handle: HANDLE,
    event: HANDLE,
    offset: u64,
    buffer: &mut AlignedBuffer,
    timeout_ms: u32,
) -> std::result::Result<usize, u32> {
    unsafe {
        let mut overlapped: OVERLAPPED = std::mem::zeroed();
        overlapped.Anonymous.Anonymous.Offset = (offset & 0xFFFFFFFF) as u32;
        overlapped.Anonymous.Anonymous.OffsetHigh = ((offset >> 32) & 0xFFFFFFFF) as u32;
        overlapped.hEvent = event;

        let mut bytes_read = 0u32;
        let res = ReadFile(
            handle,
            buffer.as_mut_ptr() as *mut _,
            buffer.len() as u32,
            &mut bytes_read,
            &mut overlapped,
        );

        if res != 0 {
            return Ok(bytes_read as usize);
        }

        let err = GetLastError();
        if err != ERROR_IO_PENDING {
            return Err(err);
        }

        let wait_res = WaitForSingleObject(event, timeout_ms);
        if wait_res == WAIT_OBJECT_0 {
            let mut transferred = 0u32;
            let success = GetOverlappedResult(handle, &mut overlapped, &mut transferred, 0);
            if success != 0 {
                return Ok(transferred as usize);
            } else {
                return Err(GetLastError());
            }
        }

        // Timeout hit - abort hardware request via CancelIoEx
        if wait_res == WAIT_TIMEOUT {
            CancelIoEx(handle, &mut overlapped);
            let mut transferred = 0u32;
            let _ = GetOverlappedResult(handle, &mut overlapped, &mut transferred, 0);
            return Err(WAIT_TIMEOUT);
        }

        CancelIoEx(handle, &mut overlapped);
        Err(GetLastError())
    }
}

#[derive(Debug, Clone)]
pub struct NtfsPartitionInfo {
    pub partition_start: u64,
    pub cluster_size: u64,
    pub total_clusters: u64,
    pub bitmap: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct NtfsBitmapFilter {
    pub partitions: Vec<NtfsPartitionInfo>,
}

impl NtfsBitmapFilter {
    pub fn is_range_in_use(&self, offset: u64, len: u64) -> bool {
        let mut matched = false;
        for part in &self.partitions {
            let part_end = part.partition_start + part.total_clusters * part.cluster_size;
            if offset >= part.partition_start && offset < part_end {
                matched = true;
                let start_cluster = (offset - part.partition_start) / part.cluster_size;
                let end_cluster = ((offset + len.saturating_sub(1) - part.partition_start) / part.cluster_size)
                    .min(part.total_clusters.saturating_sub(1));

                for c in start_cluster..=end_cluster {
                    let byte_idx = (c / 8) as usize;
                    let bit_idx = (c % 8) as u8;
                    if byte_idx < part.bitmap.len() {
                        if (part.bitmap[byte_idx] & (1 << bit_idx)) != 0 {
                            return true;
                        }
                    } else {
                        return true;
                    }
                }
            }
        }

        if !matched {
            // Keep MBR/GPT partition tables and metadata
            true
        } else {
            false
        }
    }

    pub fn detect(h_source: HANDLE, h_event: HANDLE, total_bytes: u64) -> Option<Self> {
        let mut partitions = Vec::new();
        let candidates = [1048576u64, 65536u64, 32256u64, 0u64];
        let mut vbr_buf = AlignedBuffer::new(4096, 4096);

        for &part_start in &candidates {
            if part_start + 4096 > total_bytes {
                continue;
            }
            if read_block_with_watchdog(h_source, h_event, part_start, &mut vbr_buf, 500).is_err() {
                continue;
            }
            let sec = vbr_buf.as_slice();
            if &sec[3..7] != b"NTFS" && &sec[3..8] != b"NTFS " {
                continue;
            }

            let bytes_per_sector = u16::from_le_bytes([sec[0x0B], sec[0x0C]]) as u64;
            let sectors_per_cluster = sec[0x0D] as u64;
            if bytes_per_sector == 0 || sectors_per_cluster == 0 {
                continue;
            }

            let cluster_size = bytes_per_sector * sectors_per_cluster;
            let total_sectors = u64::from_le_bytes(sec[0x28..0x30].try_into().unwrap_or([0; 8]));
            let mft_cluster = u64::from_le_bytes(sec[0x30..0x38].try_into().unwrap_or([0; 8]));
            let total_clusters = total_sectors / sectors_per_cluster;

            let mft_byte_offset = part_start + mft_cluster * cluster_size;
            let bitmap_rec_offset = mft_byte_offset + 6 * 1024;
            if bitmap_rec_offset + 4096 > total_bytes {
                continue;
            }

            let aligned_rec_offset = (bitmap_rec_offset / 4096) * 4096;
            let delta = (bitmap_rec_offset - aligned_rec_offset) as usize;
            if read_block_with_watchdog(h_source, h_event, aligned_rec_offset, &mut vbr_buf, 500).is_err() {
                continue;
            }

            let rec_slice = &vbr_buf.as_slice()[delta..];
            if rec_slice.len() < 1024 || &rec_slice[..4] != b"FILE" {
                continue;
            }

            let first_attr = u16::from_le_bytes([rec_slice[0x14], rec_slice[0x15]]) as usize;
            let mut off = first_attr;
            let mut bitmap_data = Vec::new();

            while off + 8 <= 1024 {
                let attr_type = u32::from_le_bytes(rec_slice[off..off + 4].try_into().unwrap_or([0; 4]));
                if attr_type == 0xFFFFFFFF {
                    break;
                }
                let attr_len = u32::from_le_bytes(rec_slice[off + 4..off + 8].try_into().unwrap_or([0; 4])) as usize;
                if attr_len == 0 || off + attr_len > 1024 {
                    break;
                }

                if attr_type == 0x80 {
                    let non_resident = rec_slice[off + 8] != 0;
                    if !non_resident {
                        let val_len = u32::from_le_bytes(rec_slice[off + 16..off + 20].try_into().unwrap_or([0; 4])) as usize;
                        let val_off = u16::from_le_bytes(rec_slice[off + 20..off + 22].try_into().unwrap_or([0; 2])) as usize;
                        if off + val_off + val_len <= 1024 {
                            bitmap_data = rec_slice[off + val_off..off + val_off + val_len].to_vec();
                        }
                    } else {
                        let run_offset = u16::from_le_bytes(rec_slice[off + 0x20..off + 0x22].try_into().unwrap_or([0; 2])) as usize;
                        if off + run_offset < 1024 {
                            let runs_slice = &rec_slice[off + run_offset..off + attr_len];
                            let runs = parse_raw_data_runs(runs_slice);
                            let mut scratch = AlignedBuffer::new(64 * 1024, 4096);
                            for (cluster_len, lcn) in runs {
                                let mut read_clusters = 0u64;
                                let run_bytes = cluster_len * cluster_size;
                                while read_clusters < run_bytes {
                                    let chunk = (run_bytes - read_clusters).min(64 * 1024);
                                    let read_pos = part_start + (lcn as u64 * cluster_size) + read_clusters;
                                    if read_pos + chunk <= total_bytes {
                                        if let Ok(n) = read_block_with_watchdog(h_source, h_event, read_pos, &mut scratch, 500) {
                                            bitmap_data.extend_from_slice(&scratch.as_slice()[..n.min(chunk as usize)]);
                                            read_clusters += n as u64;
                                            continue;
                                        }
                                    }
                                    break;
                                }
                            }
                        }
                    }
                    break;
                }
                off += attr_len;
            }

            if !bitmap_data.is_empty() {
                partitions.push(NtfsPartitionInfo {
                    partition_start: part_start,
                    cluster_size,
                    total_clusters,
                    bitmap: bitmap_data,
                });
            }
        }

        if !partitions.is_empty() {
            Some(NtfsBitmapFilter { partitions })
        } else {
            None
        }
    }
}

fn parse_raw_data_runs(data: &[u8]) -> Vec<(u64, i64)> {
    let mut runs = Vec::new();
    let mut idx = 0;
    let mut prev_lcn: i64 = 0;

    while idx < data.len() {
        let b = data[idx];
        if b == 0 {
            break;
        }
        idx += 1;

        let len_bytes = (b & 0x0F) as usize;
        let offset_bytes = ((b >> 4) & 0x0F) as usize;

        if idx + len_bytes + offset_bytes > data.len() {
            break;
        }

        let mut length = 0u64;
        for i in 0..len_bytes {
            length |= (data[idx + i] as u64) << (i * 8);
        }
        idx += len_bytes;

        let mut offset_val = 0i64;
        for i in 0..offset_bytes {
            offset_val |= (data[idx + i] as i64) << (i * 8);
        }
        if offset_bytes > 0 && (data[idx + offset_bytes - 1] & 0x80) != 0 {
            for i in offset_bytes..8 {
                offset_val |= 0xFFi64 << (i * 8);
            }
        }
        idx += offset_bytes;

        let lcn = prev_lcn + offset_val;
        prev_lcn = lcn;
        runs.push((length, lcn));
    }

    runs
}

pub fn run_imaging_session(
    disk: DiskInfo,
    dest_path: PathBuf,
    mapfile_path: PathBuf,
    config: ImagerConfig,
    event_tx: Sender<ImagerEvent>,
    pause: Option<Arc<AtomicBool>>,
    cancel: Option<Arc<AtomicBool>>,
) {
    let total_bytes = disk.geometry.total_bytes;
    if let Err(e) = preflight_destination_check(&dest_path, total_bytes, config.sparse_destination) {
        let _ = event_tx.send(ImagerEvent::Failed(e.to_string()));
        return;
    }

    let parent_dir = dest_path.parent().unwrap_or(Path::new("."));
    let fs_kind = query_filesystem_kind(parent_dir);
    let is_fat32 = fs_kind == FileSystemKind::Fat32;
    let is_sparse = if config.sparse_destination && fs_kind.supports_sparse() {
        true
    } else {
        false
    };

    let mut dest_writer = match MultiPartImageWriter::new(dest_path.clone(), is_fat32, is_sparse) {
        Ok(w) => w,
        Err(e) => {
            let _ = event_tx.send(ImagerEvent::Failed(format!("Failed opening destination image: {}", e)));
            return;
        }
    };

    let mut rescue_map = RescueMap::load_or_create(total_bytes, mapfile_path);
    let _ = event_tx.send(ImagerEvent::Started {
        total_bytes,
        mode: config.mode,
        sparse: is_sparse,
    });

    let sector_size = disk.alignment.logical_sector_size as usize;
    let mut chunk_buf = AlignedBuffer::new(config.chunk_size, disk.alignment.physical_sector_size as usize);
    let mut sector_buf = AlignedBuffer::new(sector_size, disk.alignment.physical_sector_size as usize);
    let dead_filler = vec![0xDE, 0xAD, 0xBE, 0xEF];

    let mut h_source = match open_overlapped_read_handle(disk.disk_number) {
        Ok(h) => h,
        Err(e) => {
            let _ = event_tx.send(ImagerEvent::Failed(e.to_string()));
            return;
        }
    };

    let h_event = unsafe { CreateEventW(ptr::null(), 1, 0, ptr::null()) };

    // Mode 2: Detect NTFS $Bitmap for smart free-space skipping
    let ntfs_filter = if config.mode == ImagingMode::SmartFilesystem {
        NtfsBitmapFilter::detect(h_source, h_event, total_bytes)
    } else {
        None
    };

    let mut last_temp_check = Instant::now();
    let mut last_save = Instant::now();
    let mut last_progress = Instant::now();
    let mut bytes_since_last_progress = 0u64;

    // Multi-pass rescue: Pass 1 Fast, Pass 2 Scraping, Pass 3 Retry
    for current_pass in 1..=3 {
        let (_rescued, bad, pending) = rescue_map.stats();
        if current_pass > 1 && (pending == 0 && bad == 0) {
            tracing::info!("All sectors successfully rescued in earlier pass. Finishing imaging.");
            break;
        }

        rescue_map.current_pass = current_pass;
        let mut pos = 0u64;

        while pos < total_bytes {
            // In Pass 2 (Scraping) and Pass 3 (Retries), fast-forward over sectors already finished
            if current_pass > 1 {
                if let Some((BlockStatus::Finished, remaining)) = rescue_map.entry_at(pos) {
                    pos += remaining.max(1);
                    continue;
                }
            }
            if let Some(ref c) = cancel {
                if c.load(Ordering::Relaxed) {
                    tracing::info!("Imaging session cancelled by user.");
                    break;
                }
            }

            while let Some(ref p) = pause {
                if p.load(Ordering::Relaxed) {
                    if let Some(ref c) = cancel {
                        if c.load(Ordering::Relaxed) {
                            break;
                        }
                    }
                    thread::sleep(Duration::from_millis(50));
                } else {
                    break;
                }
            }

            // Periodic progress emission and mapfile persistence (runs even when skipping empty clusters)
            if last_progress.elapsed() >= Duration::from_millis(500) {
                let elapsed = last_progress.elapsed().as_secs_f64();
                let speed_mb_s = if elapsed > 0.0 {
                    (bytes_since_last_progress as f64 / (1024.0 * 1024.0)) / elapsed
                } else {
                    0.0
                };
                bytes_since_last_progress = 0;
                last_progress = Instant::now();

                let (rescued, bad, pending) = rescue_map.stats();
                let _ = event_tx.send(ImagerEvent::Progress {
                    pos,
                    rescued_bytes: rescued,
                    bad_bytes: bad,
                    pending_bytes: pending,
                    speed_mb_s,
                    pass: current_pass,
                    temp_c: None,
                });
            }

            if last_save.elapsed() >= Duration::from_secs(5) {
                last_save = Instant::now();
                let _ = rescue_map.save();
                let _ = dest_writer.flush();
            }

            // Thermal throttle check every 30 seconds
            if last_temp_check.elapsed() >= Duration::from_secs(30) {
                last_temp_check = Instant::now();
                if let Ok(telemetry_handle) = open_direct_io_handle(disk.disk_number, false) {
                    if let Some(temp) = poll_temperature(telemetry_handle.0, disk.disk_number, &disk.bus_type) {
                        if temp >= config.max_temp_c {
                            let _ = event_tx.send(ImagerEvent::ThermalPause {
                                temp_c: temp,
                                threshold_c: config.max_temp_c,
                            });
                            while let Some(current_t) = poll_temperature(telemetry_handle.0, disk.disk_number, &disk.bus_type) {
                                if current_t < config.max_temp_c {
                                    let _ = event_tx.send(ImagerEvent::ThermalResumed { temp_c: current_t });
                                    break;
                                }
                                thread::sleep(Duration::from_secs(10));
                            }
                        }
                    }
                }
            }

            match current_pass {
                1 => {
                    // Pass 1: Fast Pass in 64 KB chunks, skipping ahead 1 MB on error
                    let read_len = (config.chunk_size as u64).min(total_bytes - pos) as usize;

                    // Smart Filesystem Check: bypass reading unallocated clusters
                    if let Some(ref filter) = ntfs_filter {
                        if !filter.is_range_in_use(pos, read_len as u64) {
                            let mut skip_len = read_len as u64;
                            let max_skip = (4 * 1024 * 1024u64).min(total_bytes.saturating_sub(pos));
                            if max_skip > skip_len && !filter.is_range_in_use(pos, max_skip) {
                                skip_len = max_skip;
                            }
                            rescue_map.update_range(pos, skip_len, BlockStatus::Finished);
                            pos += skip_len;
                            continue;
                        }
                    }

                    match read_block_with_watchdog(h_source, h_event, pos, &mut chunk_buf, config.read_timeout_ms) {
                        Ok(bytes) => {
                            let slice = &chunk_buf.as_slice()[..bytes];
                            if !is_sparse || !is_all_zero(slice) {
                                let _ = dest_writer.write_at(pos, slice, is_sparse);
                            }
                            rescue_map.update_range(pos, bytes as u64, BlockStatus::Finished);
                            bytes_since_last_progress += bytes as u64;
                            pos += bytes as u64;
                        }
                        Err(err) => {
                            if err == ERROR_DEVICE_NOT_CONNECTED as u32 {
                                let _ = event_tx.send(ImagerEvent::DeviceDisconnected);
                                let _ = rescue_map.save();
                                h_source = handle_reconnect(disk.disk_number, &event_tx);
                                continue;
                            }
                            rescue_map.update_range(pos, read_len as u64, BlockStatus::NonTrimmed);
                            let skip_to = (pos + config.skip_size_on_error).min(total_bytes);
                            pos = skip_to;
                        }
                    }
                }
                2 => {
                    // Pass 2: Scraping non-trimmed ranges in single sector chunks
                    let read_len = (sector_size as u64).min(total_bytes - pos) as usize;

                    // Smart Filesystem Check
                    if let Some(ref filter) = ntfs_filter {
                        if !filter.is_range_in_use(pos, read_len as u64) {
                            rescue_map.update_range(pos, read_len as u64, BlockStatus::Finished);
                            pos += read_len as u64;
                            continue;
                        }
                    }

                    match read_block_with_watchdog(h_source, h_event, pos, &mut sector_buf, config.read_timeout_ms) {
                        Ok(bytes) => {
                            let slice = &sector_buf.as_slice()[..bytes];
                            if !is_sparse || !is_all_zero(slice) {
                                let _ = dest_writer.write_at(pos, slice, is_sparse);
                            }
                            rescue_map.update_range(pos, bytes as u64, BlockStatus::Finished);
                            bytes_since_last_progress += bytes as u64;
                            pos += bytes as u64;
                        }
                        Err(_) => {
                            rescue_map.update_range(pos, read_len as u64, BlockStatus::Bad);
                            pos += read_len as u64;
                        }
                    }
                }
                _ => {
                    // Pass 3: Fill permanently bad sectors with filler
                    let fill_len = (sector_size as u64).min(total_bytes - pos) as usize;
                    let filler_chunk: Vec<u8> = dead_filler.iter().cycle().take(fill_len).cloned().collect();
                    let _ = dest_writer.write_at(pos, &filler_chunk, is_sparse);
                    pos += fill_len as u64;
                }
            }


        }
    }

    // Set logical size and finalize
    let _ = dest_writer.finalize(total_bytes);
    let _ = rescue_map.save();
    let _ = dest_writer.flush();
    unsafe {
        CloseHandle(h_event);
        CloseHandle(h_source);
    }

    let (final_rescued, final_bad, _) = rescue_map.stats();
    let _ = event_tx.send(ImagerEvent::Completed {
        total_rescued: final_rescued,
        total_bad: final_bad,
    });
}

fn handle_reconnect(disk_number: u32, event_tx: &Sender<ImagerEvent>) -> HANDLE {
    loop {
        thread::sleep(Duration::from_secs(2));
        if let Ok(handle) = open_overlapped_read_handle(disk_number) {
            let _ = event_tx.send(ImagerEvent::DeviceReconnected);
            return handle;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_all_zero_detects_empty_and_non_empty_buffers() {
        let zeroes = vec![0u8; 4096];
        assert!(is_all_zero(&zeroes));

        let mut non_zero = zeroes.clone();
        non_zero[1234] = 0x01;
        assert!(!is_all_zero(&non_zero));
    }

    #[test]
    fn test_ntfs_bitmap_filter_cluster_filtering() {
        // Create 2 clusters: cluster 0 in use (bit 0 = 1), cluster 1 free (bit 1 = 0)
        let bitmap = vec![0b00000001];
        let filter = NtfsBitmapFilter {
            partitions: vec![NtfsPartitionInfo {
                partition_start: 1048576,
                cluster_size: 4096,
                total_clusters: 2,
                bitmap,
            }],
        };

        // Offsets before partition (e.g. MBR) must be preserved
        assert!(filter.is_range_in_use(0, 512));

        // Cluster 0 is in use
        assert!(filter.is_range_in_use(1048576, 4096));

        // Cluster 1 is free space
        assert!(!filter.is_range_in_use(1048576 + 4096, 4096));
    }

    #[test]
    fn test_multipart_image_writer_splits_fat32_chunks() {
        let temp_dir = std::env::temp_dir().join(format!("imager_test_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&temp_dir);
        let base_image = temp_dir.join("test_disk.raw");

        let mut writer = MultiPartImageWriter::new(base_image.clone(), true, false).unwrap();
        // Override chunk size to 1024 bytes for easy boundary testing
        writer.max_chunk_size = 1024;

        // Write across 2 chunks: 512 bytes at 0, 512 bytes at 1024
        writer.write_at(0, &[0xAA; 512], false).unwrap();
        writer.write_at(1024, &[0xBB; 512], false).unwrap();
        writer.finalize(2048).unwrap();

        let part1 = temp_dir.join("test_disk.raw.001");
        let part2 = temp_dir.join("test_disk.raw.002");

        assert!(part1.exists(), "Part 1 should exist");
        assert!(part2.exists(), "Part 2 should exist");

        let _ = std::fs::remove_dir_all(&temp_dir);
    }
}
