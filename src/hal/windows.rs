use std::ffi::OsStr;
use std::fs;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use std::ptr;

use windows_sys::Win32::Foundation::{
    CloseHandle, GetLastError, HANDLE, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Security::{
    GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY,
};
use windows_sys::Win32::Storage::FileSystem::{
    BusTypeAta, BusTypeNvme, BusTypeSata, BusTypeScsi, BusTypeUsb, CreateFileW, GetDiskFreeSpaceExW,
    GetVolumeInformationW, ReadFile, SetFilePointerEx, WriteFile, FILE_BEGIN, FILE_FLAG_NO_BUFFERING,
    FILE_FLAG_WRITE_THROUGH, FILE_GENERIC_READ, FILE_GENERIC_WRITE, FILE_READ_ATTRIBUTES,
    FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows_sys::Win32::System::IO::DeviceIoControl;
use windows_sys::Win32::System::Ioctl::{
    DISK_GEOMETRY_EX, IOCTL_DISK_GET_DRIVE_GEOMETRY_EX, IOCTL_STORAGE_GET_DEVICE_NUMBER,
    IOCTL_STORAGE_QUERY_PROPERTY, PropertyStandardQuery, StorageAccessAlignmentProperty,
    StorageDeviceProperty, STORAGE_ACCESS_ALIGNMENT_DESCRIPTOR, STORAGE_DEVICE_DESCRIPTOR,
    STORAGE_DEVICE_NUMBER, STORAGE_PROPERTY_QUERY,
};
use windows_sys::Win32::System::SystemInformation::GetSystemDirectoryW;
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

use crate::common::error::{AegisError, Result};
use crate::common::types::{BusType, DiskGeometry, DiskInfo, PartitionSnapshot, SectorAlignment};
use crate::hal::buffer::AlignedBuffer;

pub struct SafeHandle(pub HANDLE);

impl Drop for SafeHandle {
    fn drop(&mut self) {
        if self.0 != INVALID_HANDLE_VALUE && !self.0.is_null() {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
}

pub fn to_wide_chars(s: &str) -> Vec<u16> {
    OsStr::new(s).encode_wide().chain(std::iter::once(0)).collect()
}

/// Verify if process runs with elevated Administrator privileges
pub fn is_elevated() -> bool {
    unsafe {
        let mut token: HANDLE = ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return false;
        }

        let _guard = SafeHandle(token);
        let mut elevation = std::mem::zeroed::<TOKEN_ELEVATION>();
        let mut returned_len = 0u32;

        let res = GetTokenInformation(
            token,
            TokenElevation,
            &mut elevation as *mut _ as *mut _,
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut returned_len,
        );

        res != 0 && elevation.TokenIsElevated != 0
    }
}

/// Detect physical device number of the active Windows system/boot drive (e.g. C:)
pub fn get_system_boot_disk_number() -> Option<u32> {
    unsafe {
        let mut buffer = [0u16; 260];
        let len = GetSystemDirectoryW(buffer.as_mut_ptr(), buffer.len() as u32);
        if len == 0 || (buffer[1] != b':' as u16) {
            return None;
        }

        let drive_letter = buffer[0] as u8 as char;
        let vol_path = format!("\\\\.\\{}:", drive_letter);
        let wide_path = to_wide_chars(&vol_path);

        let handle = CreateFileW(
            wide_path.as_ptr(),
            FILE_READ_ATTRIBUTES,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            ptr::null(),
            OPEN_EXISTING,
            0,
            ptr::null_mut(),
        );

        if handle == INVALID_HANDLE_VALUE {
            return None;
        }

        let _guard = SafeHandle(handle);
        let mut dev_num = std::mem::zeroed::<STORAGE_DEVICE_NUMBER>();
        let mut bytes_returned = 0u32;

        let res = DeviceIoControl(
            handle,
            IOCTL_STORAGE_GET_DEVICE_NUMBER,
            ptr::null(),
            0,
            &mut dev_num as *mut _ as *mut _,
            std::mem::size_of::<STORAGE_DEVICE_NUMBER>() as u32,
            &mut bytes_returned,
            ptr::null_mut(),
        );

        if res != 0 {
            Some(dev_num.DeviceNumber)
        } else {
            None
        }
    }
}

/// Query hardware sector alignment properties (4Kn vs 512e)
pub fn query_sector_alignment(handle: HANDLE) -> SectorAlignment {
    unsafe {
        let mut query = STORAGE_PROPERTY_QUERY {
            PropertyId: StorageAccessAlignmentProperty,
            QueryType: PropertyStandardQuery,
            AdditionalParameters: [0],
        };

        let mut desc = std::mem::zeroed::<STORAGE_ACCESS_ALIGNMENT_DESCRIPTOR>();
        let mut bytes_returned = 0u32;

        let res = DeviceIoControl(
            handle,
            IOCTL_STORAGE_QUERY_PROPERTY,
            &mut query as *mut _ as *mut _,
            std::mem::size_of::<STORAGE_PROPERTY_QUERY>() as u32,
            &mut desc as *mut _ as *mut _,
            std::mem::size_of::<STORAGE_ACCESS_ALIGNMENT_DESCRIPTOR>() as u32,
            &mut bytes_returned,
            ptr::null_mut(),
        );

        if res != 0 && desc.BytesPerLogicalSector != 0 {
            let phys = if desc.BytesPerPhysicalSector != 0 {
                desc.BytesPerPhysicalSector
            } else {
                desc.BytesPerLogicalSector
            };
            SectorAlignment {
                logical_sector_size: desc.BytesPerLogicalSector,
                physical_sector_size: phys,
            }
        } else {
            SectorAlignment::default()
        }
    }
}

/// Query disk capacity and geometry
pub fn query_disk_geometry(handle: HANDLE) -> Option<DiskGeometry> {
    unsafe {
        let mut geom_ex = std::mem::zeroed::<DISK_GEOMETRY_EX>();
        let mut bytes_returned = 0u32;

        let res = DeviceIoControl(
            handle,
            IOCTL_DISK_GET_DRIVE_GEOMETRY_EX,
            ptr::null(),
            0,
            &mut geom_ex as *mut _ as *mut _,
            std::mem::size_of::<DISK_GEOMETRY_EX>() as u32,
            &mut bytes_returned,
            ptr::null_mut(),
        );

        if res != 0 {
            let total_bytes = geom_ex.DiskSize as u64;
            let bytes_per_sector = geom_ex.Geometry.BytesPerSector;
            let total_sectors = if bytes_per_sector > 0 {
                total_bytes / bytes_per_sector as u64
            } else {
                0
            };
            Some(DiskGeometry {
                total_bytes,
                total_sectors,
                bytes_per_sector,
            })
        } else {
            None
        }
    }
}

/// Query device descriptors (model, serial, bus type)
pub fn query_device_descriptor(handle: HANDLE) -> (String, String, BusType) {
    unsafe {
        let mut query = STORAGE_PROPERTY_QUERY {
            PropertyId: StorageDeviceProperty,
            QueryType: PropertyStandardQuery,
            AdditionalParameters: [0],
        };

        let mut buffer = [0u8; 1024];
        let mut bytes_returned = 0u32;

        let res = DeviceIoControl(
            handle,
            IOCTL_STORAGE_QUERY_PROPERTY,
            &mut query as *mut _ as *mut _,
            std::mem::size_of::<STORAGE_PROPERTY_QUERY>() as u32,
            buffer.as_mut_ptr() as *mut _,
            buffer.len() as u32,
            &mut bytes_returned,
            ptr::null_mut(),
        );

        if res == 0 || bytes_returned < std::mem::size_of::<STORAGE_DEVICE_DESCRIPTOR>() as u32 {
            return (
                "Unknown Storage Device".to_string(),
                "Unknown".to_string(),
                BusType::Unknown(0),
            );
        }

        let desc = &*(buffer.as_ptr() as *const STORAGE_DEVICE_DESCRIPTOR);

        let extract_str = |offset: u32| -> String {
            if offset != 0 && (offset as usize) < buffer.len() {
                let bytes = &buffer[offset as usize..];
                let len = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
                String::from_utf8_lossy(&bytes[..len]).trim().to_string()
            } else {
                String::new()
            }
        };

        let vendor = extract_str(desc.VendorIdOffset);
        let product = extract_str(desc.ProductIdOffset);
        let serial = extract_str(desc.SerialNumberOffset);

        let model = match (vendor.is_empty(), product.is_empty()) {
            (false, false) => format!("{} {}", vendor, product),
            (true, false) => product,
            (false, true) => vendor,
            (true, true) => "Unknown Model".to_string(),
        };

        let serial_number = if serial.is_empty() {
            "Unknown".to_string()
        } else {
            serial
        };

        #[allow(non_upper_case_globals)]
        let bus_type = match desc.BusType {
            BusTypeNvme => BusType::Nvme,
            BusTypeSata | BusTypeAta => BusType::Sata,
            BusTypeUsb => BusType::Usb,
            BusTypeScsi => BusType::Scsi,
            other => BusType::Unknown(other as u32),
        };

        (model, serial_number, bus_type)
    }
}

/// Enumerate all connected physical disks and flag system boot disk
pub fn enumerate_disks() -> Vec<DiskInfo> {
    let system_disk = get_system_boot_disk_number();
    let mut disks = Vec::new();

    // Map physical drive numbers to total used space across their mounted volumes
    let mut disk_used_map: std::collections::HashMap<u32, u64> = std::collections::HashMap::new();
    for letter in b'A'..=b'Z' {
        let drive_char = letter as char;
        let vol_path = format!("\\\\.\\{}:", drive_char);
        let wide_path = to_wide_chars(&vol_path);
        unsafe {
            let handle = CreateFileW(
                wide_path.as_ptr(),
                FILE_READ_ATTRIBUTES,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                ptr::null(),
                OPEN_EXISTING,
                0,
                ptr::null_mut(),
            );
            if handle != INVALID_HANDLE_VALUE {
                let _guard = SafeHandle(handle);
                let mut dev_num = std::mem::zeroed::<STORAGE_DEVICE_NUMBER>();
                let mut bytes_returned = 0u32;
                let res = DeviceIoControl(
                    handle,
                    IOCTL_STORAGE_GET_DEVICE_NUMBER,
                    ptr::null(),
                    0,
                    &mut dev_num as *mut _ as *mut _,
                    std::mem::size_of::<STORAGE_DEVICE_NUMBER>() as u32,
                    &mut bytes_returned,
                    ptr::null_mut(),
                );
                if res != 0 {
                    let root_str = format!("{}:\\\0", drive_char).encode_utf16().collect::<Vec<u16>>();
                    let mut free_avail = 0u64;
                    let mut total_b = 0u64;
                    let mut total_f = 0u64;
                    if GetDiskFreeSpaceExW(root_str.as_ptr(), &mut free_avail, &mut total_b, &mut total_f) != 0 {
                        let used = total_b.saturating_sub(total_f);
                        *disk_used_map.entry(dev_num.DeviceNumber).or_insert(0) += used;
                    }
                }
            }
        }
    }

    for disk_num in 0..32 {
        let path = format!("\\\\.\\PhysicalDrive{}", disk_num);
        let wide_path = to_wide_chars(&path);

        unsafe {
            let handle = CreateFileW(
                wide_path.as_ptr(),
                FILE_READ_ATTRIBUTES,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                ptr::null(),
                OPEN_EXISTING,
                0,
                ptr::null_mut(),
            );

            if handle == INVALID_HANDLE_VALUE {
                continue;
            }

            let _guard = SafeHandle(handle);
            let alignment = query_sector_alignment(handle);
            let (model, serial_number, bus_type) = query_device_descriptor(handle);
            let geometry = query_disk_geometry(handle).unwrap_or(DiskGeometry {
                total_bytes: 0,
                total_sectors: 0,
                bytes_per_sector: alignment.logical_sector_size,
            });

            let is_system_boot = system_disk == Some(disk_num);
            let used_bytes = disk_used_map.get(&disk_num).copied();

            disks.push(DiskInfo {
                disk_number: disk_num,
                device_path: path,
                model,
                serial_number,
                bus_type,
                is_system_boot,
                alignment,
                geometry,
                used_bytes,
            });
        }
    }

    disks
}

/// Open physical drive handle for Direct I/O (FILE_FLAG_NO_BUFFERING)
pub fn open_direct_io_handle(disk_number: u32, write_access: bool) -> Result<SafeHandle> {
    let path = format!("\\\\.\\PhysicalDrive{}", disk_number);
    let wide_path = to_wide_chars(&path);

    let access = if write_access {
        FILE_GENERIC_READ | FILE_GENERIC_WRITE
    } else {
        FILE_GENERIC_READ
    };

    let flags = FILE_FLAG_NO_BUFFERING | if write_access { FILE_FLAG_WRITE_THROUGH } else { 0 };

    unsafe {
        let handle = CreateFileW(
            wide_path.as_ptr(),
            access,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            ptr::null(),
            OPEN_EXISTING,
            flags,
            ptr::null_mut(),
        );

        if handle == INVALID_HANDLE_VALUE {
            let err = GetLastError();
            return Err(AegisError::DeviceOpenFailed(path, err));
        }

        Ok(SafeHandle(handle))
    }
}

/// Create a raw LBA 0-63 + Secondary GPT partition snapshot backup
pub fn create_partition_snapshot(disk_number: u32, backup_dir: &Path) -> Result<PartitionSnapshot> {
    let disks = enumerate_disks();
    let disk = disks
        .iter()
        .find(|d| d.disk_number == disk_number)
        .ok_or_else(|| AegisError::DeviceOpenFailed(format!("PhysicalDrive{}", disk_number), 2))?;

    let sector_size = disk.alignment.logical_sector_size as usize;
    let header_sectors = 64; // LBA 0 through LBA 63
    let header_bytes = header_sectors * sector_size;

    let handle_guard = open_direct_io_handle(disk_number, false)?;
    let handle = handle_guard.0;

    let mut primary_buf = AlignedBuffer::new(header_bytes, disk.alignment.physical_sector_size as usize);
    primary_buf.verify_alignment()?;

    unsafe {
        let mut bytes_read = 0u32;
        let read_res = ReadFile(
            handle,
            primary_buf.as_mut_ptr() as *mut _,
            primary_buf.len() as u32,
            &mut bytes_read,
            ptr::null_mut(),
        );

        if read_res == 0 || bytes_read != primary_buf.len() as u32 {
            let err = GetLastError();
            return Err(AegisError::SnapshotFailed(
                disk_number,
                format!("Failed reading LBA 0-63: Win32 error {}", err),
            ));
        }
    }

    // Read secondary GPT (last 33 sectors)
    let backup_sectors = 33;
    let backup_bytes = backup_sectors * sector_size;
    let mut backup_buf = AlignedBuffer::new(backup_bytes, disk.alignment.physical_sector_size as usize);
    backup_buf.verify_alignment()?;

    if disk.geometry.total_bytes >= backup_bytes as u64 {
        let offset = disk.geometry.total_bytes - backup_bytes as u64;
        let aligned_offset = (offset / sector_size as u64) * sector_size as u64;

        unsafe {
            let mut new_pos = 0i64;
            SetFilePointerEx(handle, aligned_offset as i64, &mut new_pos, FILE_BEGIN);

            let mut bytes_read = 0u32;
            let read_res = ReadFile(
                handle,
                backup_buf.as_mut_ptr() as *mut _,
                backup_buf.len() as u32,
                &mut bytes_read,
                ptr::null_mut(),
            );

            if read_res == 0 {
                tracing::warn!("Warning: secondary GPT read returned error {}", GetLastError());
            }
        }
    }

    fs::create_dir_all(backup_dir)?;
    let clean_serial = disk.serial_number.chars().filter(|c| c.is_alphanumeric()).collect::<String>();
    let filename = format!("drive_{}_{}_lba_snapshot.bin", disk_number, if clean_serial.is_empty() { "unknown" } else { &clean_serial });
    let snapshot_path = backup_dir.join(filename);

    let mut combined_data = Vec::with_capacity(primary_buf.len() + backup_buf.len());
    combined_data.extend_from_slice(primary_buf.as_slice());
    combined_data.extend_from_slice(backup_buf.as_slice());

    fs::write(&snapshot_path, &combined_data)?;

    Ok(PartitionSnapshot {
        disk_number,
        primary_headers: primary_buf.as_slice().to_vec(),
        backup_gpt: backup_buf.as_slice().to_vec(),
        snapshot_path,
    })
}

/// Restore a partition table snapshot to disk
pub fn restore_partition_snapshot(disk_number: u32, snapshot_file: &Path) -> Result<()> {
    let disks = enumerate_disks();
    let disk = disks
        .iter()
        .find(|d| d.disk_number == disk_number)
        .ok_or_else(|| AegisError::DeviceOpenFailed(format!("PhysicalDrive{}", disk_number), 2))?;

    if disk.is_system_boot {
        return Err(AegisError::SystemDiskProtected(disk_number));
    }

    let data = fs::read(snapshot_file)?;
    let sector_size = disk.alignment.logical_sector_size as usize;
    let header_bytes = 64 * sector_size;

    if data.len() < header_bytes {
        return Err(AegisError::SnapshotFailed(
            disk_number,
            "Snapshot file smaller than required LBA 0-63 header size".to_string(),
        ));
    }

    let handle_guard = open_direct_io_handle(disk_number, true)?;
    let handle = handle_guard.0;

    let mut buf = AlignedBuffer::new(header_bytes, disk.alignment.physical_sector_size as usize);
    buf.as_mut_slice().copy_from_slice(&data[..header_bytes]);

    unsafe {
        let mut new_pos = 0i64;
        SetFilePointerEx(handle, 0, &mut new_pos, FILE_BEGIN);

        let mut bytes_written = 0u32;
        let res = WriteFile(
            handle,
            buf.as_ptr() as *const _,
            buf.len() as u32,
            &mut bytes_written,
            ptr::null_mut(),
        );

        if res == 0 || bytes_written != buf.len() as u32 {
            let err = GetLastError();
            return Err(AegisError::SnapshotFailed(
                disk_number,
                format!("Failed writing LBA 0-63 during restore: Win32 error {}", err),
            ));
        }
    }

    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileSystemKind {
    Ntfs,
    ExFat,
    Fat32,
    Other,
}

impl FileSystemKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Ntfs => "NTFS",
            Self::ExFat => "exFAT",
            Self::Fat32 => "FAT32",
            Self::Other => "Unknown",
        }
    }

    pub fn supports_sparse(&self) -> bool {
        matches!(self, Self::Ntfs)
    }
}

pub fn query_filesystem_kind(path: &Path) -> FileSystemKind {
    let parent = if let Ok(abs) = std::fs::canonicalize(path) {
        abs
    } else {
        std::path::PathBuf::from("C:\\")
    };
    let root = match parent.components().next() {
        Some(std::path::Component::Prefix(prefix)) => {
            format!("{}\\\0", prefix.as_os_str().to_string_lossy())
        }
        _ => "C:\\\0".to_string(),
    };
    let wide_root: Vec<u16> = root.encode_utf16().collect();

    unsafe {
        let mut fs_name = [0u16; 64];
        let mut max_comp_len = 0u32;
        let mut flags = 0u32;
        let res = GetVolumeInformationW(
            wide_root.as_ptr(),
            ptr::null_mut(),
            0,
            ptr::null_mut(),
            &mut max_comp_len,
            &mut flags,
            fs_name.as_mut_ptr(),
            fs_name.len() as u32,
        );

        if res == 0 {
            return FileSystemKind::Other;
        }

        let name_str = String::from_utf16_lossy(&fs_name)
            .trim_matches('\0')
            .to_uppercase();

        if name_str.contains("NTFS") {
            FileSystemKind::Ntfs
        } else if name_str.contains("EXFAT") {
            FileSystemKind::ExFat
        } else if name_str.contains("FAT") {
            FileSystemKind::Fat32
        } else {
            FileSystemKind::Other
        }
    }
}

/// Retrieve UTF-16 text directly from the Windows clipboard
pub fn get_clipboard_text() -> Option<String> {
    use windows_sys::Win32::System::DataExchange::{CloseClipboard, GetClipboardData, OpenClipboard};
    use windows_sys::Win32::System::Memory::{GlobalLock, GlobalUnlock};

    const CF_UNICODETEXT: u32 = 13;

    unsafe {
        if OpenClipboard(ptr::null_mut()) == 0 {
            return None;
        }
        let handle = GetClipboardData(CF_UNICODETEXT);
        if handle.is_null() {
            CloseClipboard();
            return None;
        }
        let ptr = GlobalLock(handle) as *const u16;
        if ptr.is_null() {
            CloseClipboard();
            return None;
        }

        let mut len = 0;
        while *ptr.add(len) != 0 {
            len += 1;
        }

        let slice = std::slice::from_raw_parts(ptr, len);
        let text = String::from_utf16_lossy(slice);

        GlobalUnlock(handle);
        CloseClipboard();
        Some(text)
    }
}
