use std::ptr;
use windows_sys::Win32::Foundation::HANDLE;
use windows_sys::Win32::System::IO::DeviceIoControl;

use crate::common::error::{AegisError, Result};
use crate::core::telemetry::ata::parse_ata_smart_payload;
use crate::core::telemetry::types::{DriveTelemetry, TelemetryProtocol};

const IOCTL_SCSI_PASS_THROUGH: u32 = 0x4D004;
const SCSI_IOCTL_DATA_IN: u8 = 1;

#[repr(C)]
struct ScsiPassThroughWithBuffer {
    spt: ScsiPassThrough,
    sense_buf: [u8; 32],
    data_buf: [u8; 512],
}

#[repr(C)]
struct ScsiPassThrough {
    length: u16,
    scsi_status: u8,
    path_id: u8,
    target_id: u8,
    lun: u8,
    cdb_length: u8,
    sense_info_length: u8,
    data_in: u8,
    data_transfer_length: u32,
    time_out_value: u32,
    data_buffer_offset: usize,
    sense_info_offset: u32,
    cdb: [u8; 16],
}

pub fn query_usb_sat_smart(handle: HANDLE) -> Result<DriveTelemetry> {
    unsafe {
        let mut request = std::mem::zeroed::<ScsiPassThroughWithBuffer>();
        let spt_size = std::mem::size_of::<ScsiPassThrough>();

        request.spt.length = spt_size as u16;
        request.spt.cdb_length = 16;
        request.spt.data_in = SCSI_IOCTL_DATA_IN;
        request.spt.data_transfer_length = 512;
        request.spt.time_out_value = 5; // 5 seconds
        request.spt.sense_info_length = 32;

        let sense_offset = spt_size;
        let data_offset = spt_size + 32;
        request.spt.sense_info_offset = sense_offset as u32;
        request.spt.data_buffer_offset = data_offset;

        // SAT-16 ATA PASS-THROUGH (16)
        request.spt.cdb[0] = 0x85; // ATA PASS THROUGH (16)
        request.spt.cdb[1] = 4 << 1; // PIO Data-in
        request.spt.cdb[2] = 0x2E;  // T_DIR=1 (data-in), BYTE_BLOCK=1, T_LENGTH=2 (sectors)
        request.spt.cdb[4] = 0xD0;  // Features: SMART_READ_DATA
        request.spt.cdb[6] = 1;     // Sector Count: 1
        request.spt.cdb[8] = 1;     // LBA Low
        request.spt.cdb[10] = 0x4F; // LBA Mid (0x4F SMART_CYL_LOW)
        request.spt.cdb[12] = 0xC2; // LBA High (0xC2 SMART_CYL_HI)
        request.spt.cdb[13] = 0xA0; // Device
        request.spt.cdb[14] = 0xB0; // Command: SMART_CMD (0xB0)

        let mut bytes_returned = 0u32;
        let total_size = std::mem::size_of::<ScsiPassThroughWithBuffer>() as u32;

        let res = DeviceIoControl(
            handle,
            IOCTL_SCSI_PASS_THROUGH,
            &request as *const _ as *const _,
            total_size,
            &mut request as *mut _ as *mut _,
            total_size,
            &mut bytes_returned,
            ptr::null_mut(),
        );

        if res == 0 || request.spt.scsi_status != 0 {
            // Try SAT-12 (ATA PASS THROUGH 12, Opcode 0xA1) for USB-to-SATA bridges
            let mut request12 = std::mem::zeroed::<ScsiPassThroughWithBuffer>();
            request12.spt.length = spt_size as u16;
            request12.spt.cdb_length = 12;
            request12.spt.data_in = SCSI_IOCTL_DATA_IN;
            request12.spt.data_transfer_length = 512;
            request12.spt.time_out_value = 5;
            request12.spt.sense_info_length = 32;
            request12.spt.sense_info_offset = sense_offset as u32;
            request12.spt.data_buffer_offset = data_offset;

            request12.spt.cdb[0] = 0xA1; // ATA PASS THROUGH (12)
            request12.spt.cdb[1] = 4 << 1; // PIO Data-in
            request12.spt.cdb[2] = 0x2E;
            request12.spt.cdb[3] = 0xD0; // Features: SMART_READ_DATA
            request12.spt.cdb[4] = 1;    // Sector Count
            request12.spt.cdb[5] = 1;    // LBA Low
            request12.spt.cdb[6] = 0x4F; // LBA Mid
            request12.spt.cdb[7] = 0xC2; // LBA High
            request12.spt.cdb[8] = 0xA0; // Device
            request12.spt.cdb[9] = 0xB0; // Command: SMART_CMD

            let res12 = DeviceIoControl(
                handle,
                IOCTL_SCSI_PASS_THROUGH,
                &request12 as *const _ as *const _,
                total_size,
                &mut request12 as *mut _ as *mut _,
                total_size,
                &mut bytes_returned,
                ptr::null_mut(),
            );

            if res12 == 0 || request12.spt.scsi_status != 0 {
                return Err(AegisError::IoctlFailed(
                    "IOCTL_SCSI_PASS_THROUGH (SAT-16 & SAT-12)".to_string(),
                    windows_sys::Win32::Foundation::GetLastError(),
                ));
            }

            let mut telemetry = parse_ata_smart_payload(&request12.data_buf)?;
            telemetry.protocol = Some(TelemetryProtocol::ScsiPassThroughSat);
            return Ok(telemetry);
        }

        let mut telemetry = parse_ata_smart_payload(&request.data_buf)?;
        telemetry.protocol = Some(TelemetryProtocol::ScsiPassThroughSat);
        Ok(telemetry)
    }
}
