use std::ptr;
use windows_sys::Win32::Foundation::HANDLE;
use windows_sys::Win32::System::IO::DeviceIoControl;
use windows_sys::Win32::System::Ioctl::IOCTL_STORAGE_QUERY_PROPERTY;

use crate::common::error::{AegisError, Result};
use crate::core::telemetry::types::{DriveTelemetry, TelemetryProtocol};

const STORAGE_DEVICE_PROTOCOL_SPECIFIC_PROPERTY: i32 = 49;
const STORAGE_ADAPTER_PROTOCOL_SPECIFIC_PROPERTY: i32 = 48;
const STORAGE_PROTOCOL_TYPE_NVME: u32 = 3;
const NVME_DATA_TYPE_LOG_PAGE: u32 = 2;
const NVME_LOG_PAGE_HEALTH_INFO: u32 = 2;
const NVME_DATA_TYPE_IDENTIFY: u32 = 1;
const NVME_IDENTIFY_CNS_CONTROLLER: u32 = 1;

/// Query factory hardware thermal thresholds (WCTEMP and CCTEMP) from NVMe Identify Controller (CNS 01h)
pub fn query_nvme_identify_thresholds(handle: HANDLE) -> (Option<u32>, Option<u32>) {
    unsafe {
        let total_size = std::mem::size_of::<StorageProtocolDataDescriptor>() + 4096;
        let mut buffer = vec![0u8; total_size + 1024];

        let mut query = StoragePropertyQueryProtocol {
            property_id: STORAGE_DEVICE_PROTOCOL_SPECIFIC_PROPERTY,
            query_type: 0,
            protocol_type: STORAGE_PROTOCOL_TYPE_NVME,
            data_type: NVME_DATA_TYPE_IDENTIFY,
            protocol_data_request_value: NVME_IDENTIFY_CNS_CONTROLLER,
            protocol_data_request_sub_value: 0,
            protocol_data_offset: 40,
            protocol_data_length: 4096,
            fixed_protocol_return_data: 0,
            protocol_data_request_sub_value2: 0,
            protocol_data_request_sub_value3: 0,
            reserved: 0,
        };

        let mut bytes_returned = 0u32;
        let mut res = DeviceIoControl(
            handle,
            IOCTL_STORAGE_QUERY_PROPERTY,
            &query as *const _ as *const _,
            std::mem::size_of::<StoragePropertyQueryProtocol>() as u32,
            buffer.as_mut_ptr() as *mut _,
            buffer.len() as u32,
            &mut bytes_returned,
            ptr::null_mut(),
        );

        if res == 0 {
            query.property_id = STORAGE_ADAPTER_PROTOCOL_SPECIFIC_PROPERTY;
            res = DeviceIoControl(
                handle,
                IOCTL_STORAGE_QUERY_PROPERTY,
                &query as *const _ as *const _,
                std::mem::size_of::<StoragePropertyQueryProtocol>() as u32,
                buffer.as_mut_ptr() as *mut _,
                buffer.len() as u32,
                &mut bytes_returned,
                ptr::null_mut(),
            );
        }

        if res != 0 && bytes_returned >= 512 {
            let desc = &*(buffer.as_ptr() as *const StorageProtocolDataDescriptor);
            let offset = desc.protocol_data_offset as usize;
            let cand = if offset > 0 && offset + 82 <= bytes_returned as usize {
                offset
            } else if 40 + 82 <= bytes_returned as usize {
                40
            } else {
                0
            };

            if cand > 0 {
                let wctemp_k = u16::from_le_bytes([buffer[cand + 78], buffer[cand + 79]]);
                let cctemp_k = u16::from_le_bytes([buffer[cand + 80], buffer[cand + 81]]);
                let safe = if wctemp_k >= 273 && wctemp_k <= 393 {
                    Some((wctemp_k - 273) as u32)
                } else {
                    None
                };
                let crit = if cctemp_k >= 273 && cctemp_k <= 413 {
                    Some((cctemp_k - 273) as u32)
                } else {
                    None
                };
                if safe.is_some() || crit.is_some() {
                    return (safe, crit);
                }
            }
        }
        (None, None)
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
struct StoragePropertyQueryProtocol {
    property_id: i32,
    query_type: i32,
    protocol_type: u32,
    data_type: u32,
    protocol_data_request_value: u32,
    protocol_data_request_sub_value: u32,
    protocol_data_offset: u32,
    protocol_data_length: u32,
    fixed_protocol_return_data: u32,
    protocol_data_request_sub_value2: u32,
    protocol_data_request_sub_value3: u32,
    reserved: u32,
}

#[repr(C)]
struct StorageProtocolDataDescriptor {
    version: u32,
    size: u32,
    protocol_type: u32,
    data_type: u32,
    protocol_data_request_value: u32,
    protocol_data_request_sub_value: u32,
    protocol_data_offset: u32,
    protocol_data_length: u32,
    fixed_protocol_return_data: u32,
    protocol_data_request_sub_value2: u32,
    protocol_data_request_sub_value3: u32,
    reserved: u32,
}

pub fn query_nvme_health(handle: HANDLE) -> Result<DriveTelemetry> {
    unsafe {
        let total_size = std::mem::size_of::<StorageProtocolDataDescriptor>() + 512;
        let mut buffer = vec![0u8; total_size + 1024];

        // Construct 48-byte query structure
        let mut query = StoragePropertyQueryProtocol {
            property_id: STORAGE_DEVICE_PROTOCOL_SPECIFIC_PROPERTY,
            query_type: 0, // PropertyStandardQuery
            protocol_type: STORAGE_PROTOCOL_TYPE_NVME,
            data_type: NVME_DATA_TYPE_LOG_PAGE,
            protocol_data_request_value: NVME_LOG_PAGE_HEALTH_INFO,
            protocol_data_request_sub_value: 0,
            protocol_data_offset: 40, // sizeof(STORAGE_PROTOCOL_SPECIFIC_DATA)
            protocol_data_length: 512,
            fixed_protocol_return_data: 0,
            protocol_data_request_sub_value2: 0,
            protocol_data_request_sub_value3: 0,
            reserved: 0,
        };

        let mut bytes_returned = 0u32;
        let input_len = std::mem::size_of::<StoragePropertyQueryProtocol>() as u32;

        let mut res = DeviceIoControl(
            handle,
            IOCTL_STORAGE_QUERY_PROPERTY,
            &query as *const _ as *const _,
            input_len,
            buffer.as_mut_ptr() as *mut _,
            total_size as u32,
            &mut bytes_returned,
            ptr::null_mut(),
        );

        if res == 0 {
            // Try Adapter level query (48)
            query.property_id = STORAGE_ADAPTER_PROTOCOL_SPECIFIC_PROPERTY;
            res = DeviceIoControl(
                handle,
                IOCTL_STORAGE_QUERY_PROPERTY,
                &query as *const _ as *const _,
                input_len,
                buffer.as_mut_ptr() as *mut _,
                total_size as u32,
                &mut bytes_returned,
                ptr::null_mut(),
            );

            if res == 0 {
                return Err(AegisError::IoctlFailed(
                    "IOCTL_STORAGE_QUERY_PROPERTY (NVMe Protocol Specific)".to_string(),
                    windows_sys::Win32::Foundation::GetLastError(),
                ));
            }
        }

        let desc = &*(buffer.as_ptr() as *const StorageProtocolDataDescriptor);
        if desc.protocol_data_length < 512 {
            return Err(AegisError::IoctlFailed("NVMe protocol data length too short".to_string(), 0));
        }
        let proto_offset = desc.protocol_data_offset as usize;

        // Microsoft specifies data buffer is either at `desc.protocol_data_offset` or `8 + desc.protocol_data_offset`
        let candidates = [
            proto_offset,
            8 + proto_offset,
            std::mem::size_of::<StorageProtocolDataDescriptor>(),
        ];

        let thresholds = query_nvme_identify_thresholds(handle);

        for &cand in &candidates {
            if cand + 512 <= buffer.len() {
                let slice = &buffer[cand..cand + 512];
                if slice.iter().all(|&b| b == 0) {
                    continue;
                }
                let temp_k = u16::from_le_bytes([slice[1], slice[2]]);
                if temp_k >= 273 && temp_k <= 373 {
                    return parse_nvme_health_log(slice, thresholds);
                }
            }
        }

        // Buffer scan: locate the valid temperature window (0°C to 100°C / 273K..373K) with valid spare
        if bytes_returned >= 512 {
            for i in 8..(bytes_returned as usize).saturating_sub(512) {
                let slice = &buffer[i..i + 512];
                if slice.iter().all(|&b| b == 0) {
                    continue;
                }
                let temp_k = u16::from_le_bytes([slice[1], slice[2]]);
                let spare = slice[3];
                if temp_k >= 273 && temp_k <= 373 && spare > 0 && spare <= 100 {
                    return parse_nvme_health_log(slice, thresholds);
                }
            }
        }

        // Do not parse zeroed buffer as NVMe; fail so ATA/SAT fallback tiers run
        Err(AegisError::IoctlFailed("Device is not Native NVMe (zeroed response)".to_string(), 0))
    }
}

fn parse_nvme_health_log(data: &[u8], thresholds: (Option<u32>, Option<u32>)) -> Result<DriveTelemetry> {
    if data.len() < 512 {
        return Err(AegisError::IoctlFailed("NVMe log data truncated".to_string(), 0));
    }

    let critical_warning = data[0];
    let temp_k = u16::from_le_bytes([data[1], data[2]]);
    let temperature_c = if temp_k >= 273 && temp_k <= 400 {
        Some((temp_k - 273) as u32)
    } else {
        None
    };

    let available_spare_percentage = Some(data[3]);
    let wear_percentage = Some(data[5]);

    // Data read/written units are in 1000 * 512 bytes = 512,000 bytes (~0.5 MB)
    let read_units = u128::from_le_bytes(data[32..48].try_into().unwrap_or([0; 16]));
    let data_read_tb = Some((read_units as f64 * 512_000.0) / 1_000_000_000_000.0);

    let write_units = u128::from_le_bytes(data[48..64].try_into().unwrap_or([0; 16]));
    let data_written_tb = Some((write_units as f64 * 512_000.0) / 1_000_000_000_000.0);

    let power_cycles = Some(u128::from_le_bytes(data[112..128].try_into().unwrap_or([0; 16])) as u64);
    let power_on_hours = Some(u128::from_le_bytes(data[128..144].try_into().unwrap_or([0; 16])) as u64);
    let uncorrectable_sectors = Some(u128::from_le_bytes(data[160..176].try_into().unwrap_or([0; 16])) as u64);

    // Prefer factory NVMe thresholds; if absent or invalid, fallback to JEDEC/NVMe standard limits (70°C / 80°C)
    let mut safe = thresholds.0.unwrap_or(70);
    let mut crit = thresholds.1.unwrap_or(80);
    if safe >= crit || safe < 40 || crit < 50 {
        safe = 70;
        crit = 80;
    }

    Ok(DriveTelemetry {
        protocol: Some(TelemetryProtocol::NativeNvme),
        temperature_c,
        power_on_hours,
        power_cycles,
        reallocated_sectors: None,
        pending_sectors: None,
        uncorrectable_sectors,
        wear_percentage,
        available_spare_percentage,
        data_read_tb,
        data_written_tb,
        critical_warning: Some(critical_warning),
        max_safe_temp_c: Some(safe),
        critical_temp_c: Some(crit),
        attributes: Vec::new(),
    })
}
