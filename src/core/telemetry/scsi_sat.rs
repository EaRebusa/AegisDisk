use std::ptr;
use windows_sys::Win32::Foundation::HANDLE;
use windows_sys::Win32::System::IO::DeviceIoControl;

use crate::common::error::{AegisError, Result};
use crate::core::telemetry::ata::parse_ata_smart_payload;
use crate::core::telemetry::types::{DriveTelemetry, TelemetryProtocol};

const IOCTL_SCSI_PASS_THROUGH: u32 = 0x4D004;
const IOCTL_SCSI_PASS_THROUGH_DIRECT: u32 = 0x4D014;
const SCSI_IOCTL_DATA_IN: u8 = 1;

#[repr(C)]
struct ScsiPassThroughDirect {
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
    data_buffer: *mut u8,
    sense_info_offset: u32,
    cdb: [u8; 16],
}

#[repr(C)]
struct ScsiPassThroughDirectWithSense {
    sptd: ScsiPassThroughDirect,
    sense_buf: [u8; 64],
}

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
        // Strategy 1: IOCTL_SCSI_PASS_THROUGH_DIRECT with SAT-16 (Opcode 0x85)
        let mut data_buf = [0u8; 512];
        let mut sptdw = std::mem::zeroed::<ScsiPassThroughDirectWithSense>();
        let sptd_size = std::mem::size_of::<ScsiPassThroughDirect>();

        sptdw.sptd.length = sptd_size as u16;
        sptdw.sptd.cdb_length = 16;
        sptdw.sptd.data_in = SCSI_IOCTL_DATA_IN;
        sptdw.sptd.data_transfer_length = 512;
        sptdw.sptd.time_out_value = 5;
        sptdw.sptd.data_buffer = data_buf.as_mut_ptr();
        sptdw.sptd.sense_info_length = 64;
        sptdw.sptd.sense_info_offset = sptd_size as u32;

        // SAT-16: ATA PASS-THROUGH (16)
        sptdw.sptd.cdb[0] = 0x85;
        sptdw.sptd.cdb[1] = 4 << 1; // PIO Data-in
        sptdw.sptd.cdb[2] = 0x0E;   // T_DIR=1 (data-in), BYTE_BLOCK=1, T_LENGTH=2 (sectors), CK_COND=0
        sptdw.sptd.cdb[4] = 0xD0;   // Features: SMART_READ_DATA
        sptdw.sptd.cdb[6] = 1;      // Sector Count: 1
        sptdw.sptd.cdb[8] = 0;      // LBA Low
        sptdw.sptd.cdb[10] = 0x4F;  // LBA Mid (0x4F SMART_CYL_LOW)
        sptdw.sptd.cdb[12] = 0xC2;  // LBA High (0xC2 SMART_CYL_HI)
        sptdw.sptd.cdb[13] = 0xA0;  // Device
        sptdw.sptd.cdb[14] = 0xB0;  // Command: SMART_CMD (0xB0)

        let mut bytes_returned = 0u32;
        let total_size = std::mem::size_of::<ScsiPassThroughDirectWithSense>() as u32;

        let res = DeviceIoControl(
            handle,
            IOCTL_SCSI_PASS_THROUGH_DIRECT,
            &sptdw as *const _ as *const _,
            total_size,
            &mut sptdw as *mut _ as *mut _,
            total_size,
            &mut bytes_returned,
            ptr::null_mut(),
        );

        if res != 0 && (sptdw.sptd.scsi_status == 0 || sptdw.sptd.scsi_status == 2) {
            if let Ok(mut telemetry) = parse_ata_smart_payload(&data_buf) {
                telemetry.protocol = Some(TelemetryProtocol::ScsiPassThroughSat);
                return Ok(telemetry);
            }
        }

        // Strategy 2: IOCTL_SCSI_PASS_THROUGH_DIRECT with SAT-12 (Opcode 0xA1)
        data_buf.fill(0);
        let mut sptdw12 = std::mem::zeroed::<ScsiPassThroughDirectWithSense>();
        sptdw12.sptd.length = sptd_size as u16;
        sptdw12.sptd.cdb_length = 12;
        sptdw12.sptd.data_in = SCSI_IOCTL_DATA_IN;
        sptdw12.sptd.data_transfer_length = 512;
        sptdw12.sptd.time_out_value = 5;
        sptdw12.sptd.data_buffer = data_buf.as_mut_ptr();
        sptdw12.sptd.sense_info_length = 64;
        sptdw12.sptd.sense_info_offset = sptd_size as u32;

        sptdw12.sptd.cdb[0] = 0xA1; // ATA PASS THROUGH (12)
        sptdw12.sptd.cdb[1] = 4 << 1;
        sptdw12.sptd.cdb[2] = 0x0E;
        sptdw12.sptd.cdb[3] = 0xD0;
        sptdw12.sptd.cdb[4] = 1;
        sptdw12.sptd.cdb[5] = 0;
        sptdw12.sptd.cdb[6] = 0x4F;
        sptdw12.sptd.cdb[7] = 0xC2;
        sptdw12.sptd.cdb[8] = 0xA0;
        sptdw12.sptd.cdb[9] = 0xB0;

        let res12 = DeviceIoControl(
            handle,
            IOCTL_SCSI_PASS_THROUGH_DIRECT,
            &sptdw12 as *const _ as *const _,
            total_size,
            &mut sptdw12 as *mut _ as *mut _,
            total_size,
            &mut bytes_returned,
            ptr::null_mut(),
        );

        if res12 != 0 && (sptdw12.sptd.scsi_status == 0 || sptdw12.sptd.scsi_status == 2) {
            if let Ok(mut telemetry) = parse_ata_smart_payload(&data_buf) {
                telemetry.protocol = Some(TelemetryProtocol::ScsiPassThroughSat);
                return Ok(telemetry);
            }
        }

        // Strategy 3: Fallback to buffered IOCTL_SCSI_PASS_THROUGH (SAT-16)
        let mut request = std::mem::zeroed::<ScsiPassThroughWithBuffer>();
        let spt_size = std::mem::size_of::<ScsiPassThrough>();

        request.spt.length = spt_size as u16;
        request.spt.cdb_length = 16;
        request.spt.data_in = SCSI_IOCTL_DATA_IN;
        request.spt.data_transfer_length = 512;
        request.spt.time_out_value = 5;
        request.spt.sense_info_length = 32;

        let sense_offset = spt_size;
        let data_offset = spt_size + 32;
        request.spt.sense_info_offset = sense_offset as u32;
        request.spt.data_buffer_offset = data_offset;

        request.spt.cdb[0] = 0x85;
        request.spt.cdb[1] = 4 << 1;
        request.spt.cdb[2] = 0x0E;
        request.spt.cdb[4] = 0xD0;
        request.spt.cdb[6] = 1;
        request.spt.cdb[8] = 0;
        request.spt.cdb[10] = 0x4F;
        request.spt.cdb[12] = 0xC2;
        request.spt.cdb[13] = 0xA0;
        request.spt.cdb[14] = 0xB0;

        let total_size_buf = std::mem::size_of::<ScsiPassThroughWithBuffer>() as u32;

        let res_buf = DeviceIoControl(
            handle,
            IOCTL_SCSI_PASS_THROUGH,
            &request as *const _ as *const _,
            total_size_buf,
            &mut request as *mut _ as *mut _,
            total_size_buf,
            &mut bytes_returned,
            ptr::null_mut(),
        );

        if res_buf != 0 && (request.spt.scsi_status == 0 || request.spt.scsi_status == 2) {
            if let Ok(mut telemetry) = parse_ata_smart_payload(&request.data_buf) {
                telemetry.protocol = Some(TelemetryProtocol::ScsiPassThroughSat);
                return Ok(telemetry);
            }
        }

        Err(AegisError::IoctlFailed(
            "IOCTL_SCSI_PASS_THROUGH (SAT-16 & SAT-12)".to_string(),
            windows_sys::Win32::Foundation::GetLastError(),
        ))
    }
}
