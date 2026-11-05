use std::ptr;
use windows_sys::Win32::Foundation::HANDLE;
use windows_sys::Win32::System::IO::DeviceIoControl;

use crate::common::error::{AegisError, Result};
use crate::core::telemetry::types::{DriveTelemetry, SmartAttribute, TelemetryProtocol};

const SMART_RCV_DRIVE_DATA: u32 = 0x7C088;
const SMART_CMD: u8 = 0xB0;
const SMART_READ_DATA: u8 = 0xD0;
const SMART_READ_THRESHOLDS: u8 = 0xD1;
const SMART_CYL_LOW: u8 = 0x4F;
const SMART_CYL_HI: u8 = 0xC2;

#[repr(C, packed)]
struct Ideregs {
    b_features_reg: u8,
    b_sector_count_reg: u8,
    b_sector_number_reg: u8,
    b_cyl_low_reg: u8,
    b_cyl_high_reg: u8,
    b_drive_head_reg: u8,
    b_command_reg: u8,
    b_reserved: u8,
}

#[repr(C, packed)]
struct SendCmdInParams {
    c_buffer_size: u32,
    ir_drive_regs: Ideregs,
    b_drive_number: u8,
    b_reserved: [u8; 3],
    dw_reserved: [u32; 4],
    b_buffer: [u8; 1],
}

#[repr(C, packed)]
struct DriverStatus {
    b_driver_error: u8,
    b_ide_error: u8,
    b_reserved: [u8; 2],
    dw_reserved: [u32; 2],
}

#[repr(C, packed)]
struct SendCmdOutParams {
    c_buffer_size: u32,
    driver_status: DriverStatus,
    b_buffer: [u8; 512],
}

const IOCTL_ATA_PASS_THROUGH_DIRECT: u32 = 0x4D030;
const ATA_FLAGS_DATA_IN: u16 = 0x02;

#[repr(C)]
struct AtaPassThroughDirect {
    length: u16,
    ata_flags: u16,
    path_id: u8,
    target_id: u8,
    lun: u8,
    reserved_as_uchar: u8,
    data_transfer_length: u32,
    time_out_value: u32,
    reserved_as_ulong: u32,
    data_buffer: *mut u8,
    previous_task_file: [u8; 8],
    current_task_file: [u8; 8],
}

pub fn query_ata_pass_through_smart(handle: HANDLE) -> Result<DriveTelemetry> {
    unsafe {
        let mut buffer = [0u8; 512];
        let mut aptd = std::mem::zeroed::<AtaPassThroughDirect>();
        aptd.length = std::mem::size_of::<AtaPassThroughDirect>() as u16;
        aptd.ata_flags = ATA_FLAGS_DATA_IN;
        aptd.data_transfer_length = 512;
        aptd.time_out_value = 5;
        aptd.data_buffer = buffer.as_mut_ptr();

        // ATA SMART READ DATA registers
        aptd.current_task_file[0] = SMART_READ_DATA; // 0xD0 (features)
        aptd.current_task_file[1] = 1;               // sector count
        aptd.current_task_file[2] = 1;               // sector number / LBA low
        aptd.current_task_file[3] = SMART_CYL_LOW;   // 0x4F
        aptd.current_task_file[4] = SMART_CYL_HI;    // 0xC2
        aptd.current_task_file[5] = 0xA0;            // device head
        aptd.current_task_file[6] = SMART_CMD;       // 0xB0 command

        let mut bytes_returned = 0u32;
        let res = DeviceIoControl(
            handle,
            IOCTL_ATA_PASS_THROUGH_DIRECT,
            &aptd as *const _ as *const _,
            std::mem::size_of::<AtaPassThroughDirect>() as u32,
            &mut aptd as *mut _ as *mut _,
            std::mem::size_of::<AtaPassThroughDirect>() as u32,
            &mut bytes_returned,
            ptr::null_mut(),
        );

        if res == 0 {
            return Err(AegisError::IoctlFailed(
                "IOCTL_ATA_PASS_THROUGH_DIRECT".to_string(),
                windows_sys::Win32::Foundation::GetLastError(),
            ));
        }

        let mut t = parse_ata_smart_payload(&buffer)?;
        t.protocol = Some(TelemetryProtocol::AtaPassThrough);
        Ok(t)
    }
}

pub fn query_legacy_ata_smart(handle: HANDLE, drive_number: u8) -> Result<DriveTelemetry> {
    // Try drive 0 first (standard for PhysicalDrive handles)
    if let Ok(t) = try_legacy_ata_smart_with_drive(handle, 0) {
        return Ok(t);
    }
    // Try explicit drive_number if non-zero
    if drive_number != 0 {
        if let Ok(t) = try_legacy_ata_smart_with_drive(handle, drive_number) {
            return Ok(t);
        }
    }
    unsafe {
        Err(AegisError::IoctlFailed(
            "SMART_RCV_DRIVE_DATA".to_string(),
            windows_sys::Win32::Foundation::GetLastError(),
        ))
    }
}

fn try_legacy_ata_smart_with_drive(handle: HANDLE, drive_number: u8) -> Result<DriveTelemetry> {
    unsafe {
        let mut in_params = std::mem::zeroed::<SendCmdInParams>();
        in_params.c_buffer_size = 512;
        in_params.b_drive_number = drive_number;
        in_params.ir_drive_regs.b_features_reg = SMART_READ_DATA;
        in_params.ir_drive_regs.b_sector_count_reg = 1;
        in_params.ir_drive_regs.b_sector_number_reg = 1;
        in_params.ir_drive_regs.b_cyl_low_reg = SMART_CYL_LOW;
        in_params.ir_drive_regs.b_cyl_high_reg = SMART_CYL_HI;
        in_params.ir_drive_regs.b_drive_head_reg = 0xA0;
        in_params.ir_drive_regs.b_command_reg = SMART_CMD;

        let mut out_params = std::mem::zeroed::<SendCmdOutParams>();
        let mut bytes_returned = 0u32;

        let res = DeviceIoControl(
            handle,
            SMART_RCV_DRIVE_DATA,
            &in_params as *const _ as *const _,
            std::mem::size_of::<SendCmdInParams>() as u32,
            &mut out_params as *mut _ as *mut _,
            std::mem::size_of::<SendCmdOutParams>() as u32,
            &mut bytes_returned,
            ptr::null_mut(),
        );

        if res == 0 {
            return Err(AegisError::IoctlFailed(
                "SMART_RCV_DRIVE_DATA".to_string(),
                windows_sys::Win32::Foundation::GetLastError(),
            ));
        }

        let thresholds = query_ata_smart_thresholds(handle, drive_number);
        parse_ata_smart_payload_with_thresholds(&out_params.b_buffer, &thresholds)
    }
}

fn query_ata_smart_thresholds(handle: HANDLE, drive_number: u8) -> Vec<(u8, u8)> {
    unsafe {
        let mut in_params = std::mem::zeroed::<SendCmdInParams>();
        in_params.c_buffer_size = 512;
        in_params.b_drive_number = drive_number;
        in_params.ir_drive_regs.b_features_reg = SMART_READ_THRESHOLDS;
        in_params.ir_drive_regs.b_sector_count_reg = 1;
        in_params.ir_drive_regs.b_sector_number_reg = 1;
        in_params.ir_drive_regs.b_cyl_low_reg = SMART_CYL_LOW;
        in_params.ir_drive_regs.b_cyl_high_reg = SMART_CYL_HI;
        in_params.ir_drive_regs.b_drive_head_reg = 0xA0;
        in_params.ir_drive_regs.b_command_reg = SMART_CMD;

        let mut out_params = std::mem::zeroed::<SendCmdOutParams>();
        let mut bytes_returned = 0u32;

        let res = DeviceIoControl(
            handle,
            SMART_RCV_DRIVE_DATA,
            &in_params as *const _ as *const _,
            std::mem::size_of::<SendCmdInParams>() as u32,
            &mut out_params as *mut _ as *mut _,
            std::mem::size_of::<SendCmdOutParams>() as u32,
            &mut bytes_returned,
            ptr::null_mut(),
        );

        let mut result = Vec::new();
        if res != 0 && bytes_returned >= 512 {
            let mut offset = 2;
            while offset + 12 <= 362 {
                let id = out_params.b_buffer[offset];
                let thresh = out_params.b_buffer[offset + 1];
                if id != 0 {
                    result.push((id, thresh));
                }
                offset += 12;
            }
        }
        result
    }
}

pub fn parse_ata_smart_payload(raw: &[u8; 512]) -> Result<DriveTelemetry> {
    parse_ata_smart_payload_with_thresholds(raw, &[])
}

pub fn parse_ata_smart_payload_with_thresholds(raw: &[u8; 512], thresholds: &[(u8, u8)]) -> Result<DriveTelemetry> {
    let mut telemetry = DriveTelemetry {
        protocol: Some(TelemetryProtocol::LegacyAtaSmart),
        max_safe_temp_c: Some(50),
        critical_temp_c: Some(60),
        ..Default::default()
    };

    // If manufacturer provided a threshold for Attribute 0xC2 (Temperature), apply it
    if let Some(&(_, thresh)) = thresholds.iter().find(|(id, _)| *id == 0xC2) {
        if thresh > 40 && thresh <= 100 {
            telemetry.critical_temp_c = Some(thresh as u32);
            telemetry.max_safe_temp_c = Some((thresh as u32).saturating_sub(10).max(40));
        }
    }

    // SMART attributes start at byte 2, up to 30 attributes (12 bytes each)
    let mut offset = 2;
    while offset + 12 <= 362 {
        let attr_data = &raw[offset..offset + 12];
        let id = attr_data[0];
        if id != 0 {
            let current = attr_data[3];
            let worst = attr_data[4];
            let mut raw_bytes = [0u8; 8];
            raw_bytes[..6].copy_from_slice(&attr_data[5..11]);
            let raw_value = u64::from_le_bytes(raw_bytes);

            let (name, is_critical) = get_smart_name(id);

            match id {
                0x05 => telemetry.reallocated_sectors = Some(raw_value),
                0x09 => telemetry.power_on_hours = Some(raw_value),
                0x0C => telemetry.power_cycles = Some(raw_value),
                0xBB => telemetry.uncorrectable_sectors = Some(raw_value),
                0xC2 => {
                    // Temperature is usually lowest byte or word
                    telemetry.temperature_c = Some((raw_value & 0xFF) as u32);
                }
                0xC5 => telemetry.pending_sectors = Some(raw_value),
                0xE7 => {
                    // SSD Life Left / Wear
                    if current <= 100 {
                        telemetry.wear_percentage = Some(100 - current);
                    }
                }
                _ => {}
            }

            let thresh_val = thresholds.iter().find(|(t_id, _)| *t_id == id).map(|(_, v)| *v).unwrap_or(0);
            let is_failing = is_critical && thresh_val > 0 && current <= thresh_val;

            telemetry.attributes.push(SmartAttribute {
                id,
                name: name.to_string(),
                current,
                worst,
                threshold: thresh_val,
                raw_value,
                is_failing,
            });
        }
        offset += 12;
    }

    if telemetry.attributes.is_empty() {
        return Err(AegisError::IoctlFailed("Empty or invalid ATA SMART payload".to_string(), 0));
    }

    Ok(telemetry)
}

fn get_smart_name(id: u8) -> (&'static str, bool) {
    match id {
        0x01 => ("Raw Read Error Rate", true),
        0x05 => ("Reallocated Sectors Count", true),
        0x07 => ("Seek Error Rate", false),
        0x09 => ("Power-On Hours", false),
        0x0A => ("Spin Retry Count", true),
        0x0C => ("Power Cycle Count", false),
        0xBB => ("Reported Uncorrectable", true),
        0xBC => ("Command Timeout", false),
        0xC2 => ("Temperature", false),
        0xC4 => ("Reallocation Event Count", true),
        0xC5 => ("Current Pending Sector", true),
        0xC6 => ("Offline Uncorrectable", true),
        0xC7 => ("UltraDMA CRC Error", false),
        0xE7 => ("SSD Life Remaining", false),
        0xF1 => ("Total LBAs Written", false),
        0xF2 => ("Total LBAs Read", false),
        _ => ("Vendor Specific", false),
    }
}
