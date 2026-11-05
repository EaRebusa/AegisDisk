use std::ptr;
use std::time::Instant;
use windows_sys::Win32::Foundation::{GetLastError, HANDLE};
use windows_sys::Win32::Storage::FileSystem::{ReadFile, SetFilePointerEx, FILE_BEGIN};

use crate::hal::buffer::AlignedBuffer;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LatencyBucket {
    Fast,      // < 10ms
    Normal,    // 10ms .. 50ms
    Sluggish,  // 50ms .. 150ms
    Critical,  // 150ms .. 500ms
    Timeout,   // >= 500ms
    ReadError(u32),
}

impl LatencyBucket {
    pub fn symbol(&self) -> &'static str {
        match self {
            Self::Fast => "■",
            Self::Normal => "■",
            Self::Sluggish => "■",
            Self::Critical => "■",
            Self::Timeout => "■",
            Self::ReadError(_) => "X",
        }
    }

    pub fn color_code(&self) -> &'static str {
        match self {
            Self::Fast => "\x1b[32m",       // Green
            Self::Normal => "\x1b[92m",     // Light green
            Self::Sluggish => "\x1b[33m",   // Yellow
            Self::Critical => "\x1b[38;5;208m", // Orange
            Self::Timeout => "\x1b[31m",    // Red
            Self::ReadError(_) => "\x1b[35m", // Purple
        }
    }
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct BlockScanResult {
    pub block_index: u64,
    pub byte_offset: u64,
    pub duration_ms: f64,
    pub bucket: LatencyBucket,
}

#[derive(Debug, Clone, Default)]
pub struct SurfaceScanStats {
    pub fast_count: u64,
    pub normal_count: u64,
    pub sluggish_count: u64,
    pub critical_count: u64,
    pub timeout_count: u64,
    pub error_count: u64,
    pub total_blocks: u64,
    pub total_bytes_scanned: u64,
}

impl SurfaceScanStats {
    pub fn record(&mut self, bucket: LatencyBucket, bytes: usize) {
        self.total_blocks += 1;
        self.total_bytes_scanned += bytes as u64;
        match bucket {
            LatencyBucket::Fast => self.fast_count += 1,
            LatencyBucket::Normal => self.normal_count += 1,
            LatencyBucket::Sluggish => self.sluggish_count += 1,
            LatencyBucket::Critical => self.critical_count += 1,
            LatencyBucket::Timeout => self.timeout_count += 1,
            LatencyBucket::ReadError(_) => self.error_count += 1,
        }
    }
}

/// Read a single aligned block and benchmark latency
pub fn scan_single_block(
    handle: HANDLE,
    byte_offset: u64,
    buffer: &mut AlignedBuffer,
) -> BlockScanResult {
    let start = Instant::now();
    unsafe {
        let mut new_pos = 0i64;
        let seek_ok = SetFilePointerEx(handle, byte_offset as i64, &mut new_pos, FILE_BEGIN);

        if seek_ok == 0 {
            let err = GetLastError();
            return BlockScanResult {
                block_index: byte_offset / buffer.len() as u64,
                byte_offset,
                duration_ms: start.elapsed().as_secs_f64() * 1000.0,
                bucket: LatencyBucket::ReadError(err),
            };
        }

        let mut bytes_read = 0u32;
        let read_ok = ReadFile(
            handle,
            buffer.as_mut_ptr() as *mut _,
            buffer.len() as u32,
            &mut bytes_read,
            ptr::null_mut(),
        );

        let duration_ms = start.elapsed().as_secs_f64() * 1000.0;

        if read_ok == 0 || bytes_read != buffer.len() as u32 {
            let err = GetLastError();
            BlockScanResult {
                block_index: byte_offset / buffer.len() as u64,
                byte_offset,
                duration_ms,
                bucket: LatencyBucket::ReadError(err),
            }
        } else {
            let bucket = if duration_ms < 10.0 {
                LatencyBucket::Fast
            } else if duration_ms < 50.0 {
                LatencyBucket::Normal
            } else if duration_ms < 150.0 {
                LatencyBucket::Sluggish
            } else if duration_ms < 500.0 {
                LatencyBucket::Critical
            } else {
                LatencyBucket::Timeout
            };

            BlockScanResult {
                block_index: byte_offset / buffer.len() as u64,
                byte_offset,
                duration_ms,
                bucket,
            }
        }
    }
}
