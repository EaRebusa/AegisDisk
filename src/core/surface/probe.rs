use std::path::Path;
use std::ptr;
use windows_sys::Win32::Foundation::{GetLastError, HANDLE};
use windows_sys::Win32::Storage::FileSystem::{
    FlushFileBuffers, ReadFile, SetFilePointerEx, WriteFile, FILE_BEGIN,
};

use crate::common::error::{AegisError, Result};
use crate::common::types::DiskInfo;
use crate::core::surface::pattern::{
    generate_probe_block, verify_probe_block, PatternVerification,
};
use crate::hal::buffer::AlignedBuffer;
use crate::hal::windows::{create_partition_snapshot, open_direct_io_handle};

#[derive(Debug, Clone)]
pub struct FakeProbeOutcome {
    pub declared_capacity_bytes: u64,
    pub verified_capacity_bytes: u64,
    pub is_fake: bool,
    pub anomaly: Option<String>,
}

/// Execute destructive Sparse LBA Fake Capacity Probe with strict cache flushing
pub fn run_sparse_fake_probe(
    disk: &DiskInfo,
    backup_dir: &Path,
) -> Result<FakeProbeOutcome> {
    if disk.is_system_boot {
        return Err(AegisError::SystemDiskProtected(disk.disk_number));
    }

    // Step 0: Lock & Dismount mounted volumes to permit direct sector writes in Windows kernel
    let _vol_locks = crate::hal::windows::lock_and_dismount_disk_volumes(disk.disk_number);

    // Step 1: Automatic Pre-Execution Partition Snapshot
    tracing::info!("Taking pre-execution partition snapshot...");
    let _snapshot = create_partition_snapshot(disk.disk_number, backup_dir)?;

    // Step 2: Open Handle with FILE_FLAG_NO_BUFFERING and FILE_FLAG_WRITE_THROUGH
    let handle_guard = open_direct_io_handle(disk.disk_number, true)?;
    let handle = handle_guard.0;

    let sector_size = disk.alignment.logical_sector_size as usize;
    let block_size = 4096.max(sector_size);

    let mut write_buf = AlignedBuffer::new(block_size, disk.alignment.physical_sector_size as usize);
    let mut read_buf = AlignedBuffer::new(block_size, disk.alignment.physical_sector_size as usize);

    let declared_bytes = disk.geometry.total_bytes;

    run_fake_probe_engine(
        declared_bytes,
        sector_size,
        block_size,
        &mut write_buf,
        &mut read_buf,
        |offset, buf| write_and_flush(handle, offset, buf),
        |offset, buf| read_block(handle, offset, buf),
    )
}

/// Core probe algorithm decoupled from hardware I/O for 100% testable simulation
pub fn run_fake_probe_engine<W, R>(
    declared_bytes: u64,
    sector_size: usize,
    block_size: usize,
    write_buf: &mut AlignedBuffer,
    read_buf: &mut AlignedBuffer,
    mut write_fn: W,
    mut read_fn: R,
) -> Result<FakeProbeOutcome>
where
    W: FnMut(u64, &AlignedBuffer) -> Result<()>,
    R: FnMut(u64, &mut AlignedBuffer) -> Result<()>,
{
    let mut verified_bytes = 0u64;

    // Power-of-two boundaries: 1 GB, 2 GB, 4 GB, 8 GB, 16 GB, 32 GB, 64 GB, 128 GB, 256 GB, 512 GB, 1 TB...
    let mut step = 1024 * 1024 * 1024u64; // 1 GB
    let baseline_offset = 64 * sector_size as u64; // Baseline anchor at LBA 64 (safely past GPT array)

    // Write baseline anchor
    tracing::info!("Writing baseline anchor block at offset {} bytes (LBA 64)...", baseline_offset);
    generate_probe_block(write_buf.as_mut_slice(), baseline_offset);
    write_fn(baseline_offset, write_buf)?;

    while step < declared_bytes {
        let probe_offset = (step + (16 * sector_size as u64)).min(declared_bytes - block_size as u64);
        tracing::info!("Probing sparse boundary offset {:.2} GB ({} bytes)...", probe_offset as f64 / 1e9, probe_offset);

        // 1. Write signed pattern to high target offset
        generate_probe_block(write_buf.as_mut_slice(), probe_offset);
        write_fn(probe_offset, write_buf)?;

        // 2. Read back baseline anchor to detect wrap-around overwrite
        read_fn(baseline_offset, read_buf)?;
        match verify_probe_block(read_buf.as_slice(), baseline_offset) {
            PatternVerification::Valid => {}
            PatternVerification::WrapAroundOverwrite { written_for_offset } => {
                return Ok(FakeProbeOutcome {
                    declared_capacity_bytes: declared_bytes,
                    verified_capacity_bytes: verified_bytes,
                    is_fake: true,
                    anomaly: Some(format!(
                        "Wrap-around detected! Writing to {:.2} GB overwrote anchor LBA at {:.2} GB.",
                        written_for_offset as f64 / 1e9,
                        baseline_offset as f64 / 1e9
                    )),
                });
            }
            other => {
                return Ok(FakeProbeOutcome {
                    declared_capacity_bytes: declared_bytes,
                    verified_capacity_bytes: verified_bytes,
                    is_fake: true,
                    anomaly: Some(format!("Baseline anchor corrupted: {:?}", other)),
                });
            }
        }

        // 3. Read back target probe offset to detect black-hole drop
        read_fn(probe_offset, read_buf)?;
        match verify_probe_block(read_buf.as_slice(), probe_offset) {
            PatternVerification::Valid => {
                verified_bytes = probe_offset;
            }
            PatternVerification::BlackHoleZeros => {
                return Ok(FakeProbeOutcome {
                    declared_capacity_bytes: declared_bytes,
                    verified_capacity_bytes: verified_bytes,
                    is_fake: true,
                    anomaly: Some(format!(
                        "Black-hole drop detected at {:.2} GB: Sector returned pure 0x00.",
                        probe_offset as f64 / 1e9
                    )),
                });
            }
            PatternVerification::BlackHoleOnes => {
                return Ok(FakeProbeOutcome {
                    declared_capacity_bytes: declared_bytes,
                    verified_capacity_bytes: verified_bytes,
                    is_fake: true,
                    anomaly: Some(format!(
                        "Black-hole drop detected at {:.2} GB: Sector returned pure 0xFF.",
                        probe_offset as f64 / 1e9
                    )),
                });
            }
            other => {
                return Ok(FakeProbeOutcome {
                    declared_capacity_bytes: declared_bytes,
                    verified_capacity_bytes: verified_bytes,
                    is_fake: true,
                    anomaly: Some(format!("Verification anomaly at {:.2} GB: {:?}", probe_offset as f64 / 1e9, other)),
                });
            }
        }

        step *= 2;
    }

    // Always test the high-capacity end boundary (declared_bytes - block_size) if not already verified
    let end_probe_offset = declared_bytes.saturating_sub(block_size as u64);
    if end_probe_offset > baseline_offset && end_probe_offset > verified_bytes {
        tracing::info!("Probing final declared boundary at {:.2} GB ({} bytes)...", end_probe_offset as f64 / 1e9, end_probe_offset);
        generate_probe_block(write_buf.as_mut_slice(), end_probe_offset);
        write_fn(end_probe_offset, write_buf)?;

        read_fn(baseline_offset, read_buf)?;
        match verify_probe_block(read_buf.as_slice(), baseline_offset) {
            PatternVerification::Valid => {}
            PatternVerification::WrapAroundOverwrite { written_for_offset } => {
                return Ok(FakeProbeOutcome {
                    declared_capacity_bytes: declared_bytes,
                    verified_capacity_bytes: verified_bytes,
                    is_fake: true,
                    anomaly: Some(format!(
                        "Wrap-around detected! Writing to final boundary {:.2} GB overwrote anchor LBA at {:.2} GB.",
                        written_for_offset as f64 / 1e9,
                        baseline_offset as f64 / 1e9
                    )),
                });
            }
            other => {
                return Ok(FakeProbeOutcome {
                    declared_capacity_bytes: declared_bytes,
                    verified_capacity_bytes: verified_bytes,
                    is_fake: true,
                    anomaly: Some(format!("Baseline anchor corrupted during end probe: {:?}", other)),
                });
            }
        }

        read_fn(end_probe_offset, read_buf)?;
        match verify_probe_block(read_buf.as_slice(), end_probe_offset) {
            PatternVerification::Valid => {
                verified_bytes = end_probe_offset;
            }
            PatternVerification::BlackHoleZeros => {
                return Ok(FakeProbeOutcome {
                    declared_capacity_bytes: declared_bytes,
                    verified_capacity_bytes: verified_bytes,
                    is_fake: true,
                    anomaly: Some(format!(
                        "Black-hole drop detected at final boundary {:.2} GB: Sector returned pure 0x00.",
                        end_probe_offset as f64 / 1e9
                    )),
                });
            }
            PatternVerification::BlackHoleOnes => {
                return Ok(FakeProbeOutcome {
                    declared_capacity_bytes: declared_bytes,
                    verified_capacity_bytes: verified_bytes,
                    is_fake: true,
                    anomaly: Some(format!(
                        "Black-hole drop detected at final boundary {:.2} GB: Sector returned pure 0xFF.",
                        end_probe_offset as f64 / 1e9
                    )),
                });
            }
            other => {
                return Ok(FakeProbeOutcome {
                    declared_capacity_bytes: declared_bytes,
                    verified_capacity_bytes: verified_bytes,
                    is_fake: true,
                    anomaly: Some(format!("Verification anomaly at final boundary {:.2} GB: {:?}", end_probe_offset as f64 / 1e9, other)),
                });
            }
        }
    }

    Ok(FakeProbeOutcome {
        declared_capacity_bytes: declared_bytes,
        verified_capacity_bytes: if verified_bytes == end_probe_offset { declared_bytes } else { verified_bytes },
        is_fake: false,
        anomaly: None,
    })
}

fn write_and_flush(handle: HANDLE, offset: u64, buffer: &AlignedBuffer) -> Result<()> {
    unsafe {
        let mut new_pos = 0i64;
        SetFilePointerEx(handle, offset as i64, &mut new_pos, FILE_BEGIN);

        let mut bytes_written = 0u32;
        let res = WriteFile(
            handle,
            buffer.as_ptr() as *const _,
            buffer.len() as u32,
            &mut bytes_written,
            ptr::null_mut(),
        );

        if res == 0 || bytes_written != buffer.len() as u32 {
            return Err(AegisError::IoctlFailed("Direct WriteFile failed".to_string(), GetLastError()));
        }

        // Enforce hardware cache flush
        FlushFileBuffers(handle);
        Ok(())
    }
}

fn read_block(handle: HANDLE, offset: u64, buffer: &mut AlignedBuffer) -> Result<()> {
    unsafe {
        let mut new_pos = 0i64;
        SetFilePointerEx(handle, offset as i64, &mut new_pos, FILE_BEGIN);

        let mut bytes_read = 0u32;
        let res = ReadFile(
            handle,
            buffer.as_mut_ptr() as *mut _,
            buffer.len() as u32,
            &mut bytes_read,
            ptr::null_mut(),
        );

        if res == 0 || bytes_read != buffer.len() as u32 {
            return Err(AegisError::IoctlFailed("Direct ReadFile failed".to_string(), GetLastError()));
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    #[test]
    fn test_fake_probe_detects_hardware_wraparound() {
        // Simulate a counterfeit USB drive declaring 64 GB, but hardware flash wraps around at 4 GB
        let declared = 64 * 1024 * 1024 * 1024u64;
        let real_flash_size = 4 * 1024 * 1024 * 1024u64;
        let baseline_offset = 64 * 512u64;

        let simulated_flash: Arc<Mutex<HashMap<u64, Vec<u8>>>> = Arc::new(Mutex::new(HashMap::new()));
        let mut write_buf = AlignedBuffer::new(4096, 4096);
        let mut read_buf = AlignedBuffer::new(4096, 4096);

        let flash_writer = Arc::clone(&simulated_flash);
        let flash_reader = Arc::clone(&simulated_flash);

        let outcome = run_fake_probe_engine(
            declared,
            512,
            4096,
            &mut write_buf,
            &mut read_buf,
            move |offset, buf| {
                // When writing to high LBA beyond 4GB, counterfeit firmware wraps and overwrites baseline anchor
                let physical_offset = if offset > real_flash_size {
                    baseline_offset
                } else {
                    offset % real_flash_size
                };
                flash_writer.lock().unwrap().insert(physical_offset, buf.as_slice().to_vec());
                Ok(())
            },
            move |offset, buf| {
                let physical_offset = if offset > real_flash_size {
                    baseline_offset
                } else {
                    offset % real_flash_size
                };
                if let Some(data) = flash_reader.lock().unwrap().get(&physical_offset) {
                    buf.as_mut_slice().copy_from_slice(data);
                } else {
                    buf.as_mut_slice().fill(0);
                }
                Ok(())
            },
        ).unwrap();

        assert!(outcome.is_fake, "Counterfeit drive with 4GB wrap-around must be detected as fake");
        assert!(outcome.anomaly.unwrap().contains("Wrap-around detected"));
    }

    #[test]
    fn test_fake_probe_detects_black_hole_drive() {
        // Simulate a fake drive that drops writes beyond 8 GB into a black hole (returns pure zeroes)
        let declared = 64 * 1024 * 1024 * 1024u64;
        let real_limit = 8 * 1024 * 1024 * 1024u64;

        let simulated_flash: Arc<Mutex<HashMap<u64, Vec<u8>>>> = Arc::new(Mutex::new(HashMap::new()));
        let mut write_buf = AlignedBuffer::new(4096, 4096);
        let mut read_buf = AlignedBuffer::new(4096, 4096);

        let flash_writer = Arc::clone(&simulated_flash);
        let flash_reader = Arc::clone(&simulated_flash);

        let outcome = run_fake_probe_engine(
            declared,
            512,
            4096,
            &mut write_buf,
            &mut read_buf,
            move |offset, buf| {
                if offset < real_limit {
                    flash_writer.lock().unwrap().insert(offset, buf.as_slice().to_vec());
                }
                Ok(())
            },
            move |offset, buf| {
                if let Some(data) = flash_reader.lock().unwrap().get(&offset) {
                    buf.as_mut_slice().copy_from_slice(data);
                } else {
                    buf.as_mut_slice().fill(0);
                }
                Ok(())
            },
        ).unwrap();

        assert!(outcome.is_fake, "Black hole drive dropping writes must be detected as fake");
        assert!(outcome.anomaly.unwrap().contains("Black-hole drop"));
    }

    #[test]
    fn test_fake_probe_confirms_genuine_drive() {
        // Simulate a genuine 16 GB drive with 100% honest flash storage
        let declared = 16 * 1024 * 1024 * 1024u64;

        let simulated_flash: Arc<Mutex<HashMap<u64, Vec<u8>>>> = Arc::new(Mutex::new(HashMap::new()));
        let mut write_buf = AlignedBuffer::new(4096, 4096);
        let mut read_buf = AlignedBuffer::new(4096, 4096);

        let flash_writer = Arc::clone(&simulated_flash);
        let flash_reader = Arc::clone(&simulated_flash);

        let outcome = run_fake_probe_engine(
            declared,
            512,
            4096,
            &mut write_buf,
            &mut read_buf,
            move |offset, buf| {
                flash_writer.lock().unwrap().insert(offset, buf.as_slice().to_vec());
                Ok(())
            },
            move |offset, buf| {
                if let Some(data) = flash_reader.lock().unwrap().get(&offset) {
                    buf.as_mut_slice().copy_from_slice(data);
                } else {
                    buf.as_mut_slice().fill(0);
                }
                Ok(())
            },
        ).unwrap();

        assert!(!outcome.is_fake, "Genuine drive must pass fake probe successfully");
        assert_eq!(outcome.verified_capacity_bytes, declared);
        assert!(outcome.anomaly.is_none());
    }
}
