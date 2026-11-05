pub mod ata;
pub mod nvme;
pub mod scsi_sat;
pub mod types;

use windows_sys::Win32::Foundation::HANDLE;
use crate::common::error::Result;
use crate::common::types::BusType;
#[allow(unused_imports)]
pub use types::{DriveTelemetry, SmartAttribute, TelemetryProtocol};

/// Unified hardware-agnostic telemetry query (NVMe, SATA AHCI, USB SAT-16, Legacy ATA, Generic Block)
pub fn query_telemetry(handle: HANDLE, disk_number: u32, _bus_type: &BusType) -> Result<DriveTelemetry> {
    // Tier 1: Try native NVMe Health Log Page (0x02)
    if let Ok(telemetry) = nvme::query_nvme_health(handle) {
        return Ok(telemetry);
    }

    // Tier 2: Try modern ATA Pass-Through (SATA HDDs & SSDs on AHCI)
    if let Ok(telemetry) = ata::query_ata_pass_through_smart(handle) {
        return Ok(telemetry);
    }

    // Tier 3: Try SCSI Pass-Through ATA Translation SAT-16 (USB enclosures, external bridges, RAID)
    if let Ok(telemetry) = scsi_sat::query_usb_sat_smart(handle) {
        return Ok(telemetry);
    }

    // Tier 4: Legacy ATA SMART via SMART_RCV_DRIVE_DATA
    if let Ok(telemetry) = ata::query_legacy_ata_smart(handle, disk_number as u8) {
        return Ok(telemetry);
    }

    // Tier 5: Generic Block Fallback (SD cards, USB flash drives, eMMC without SMART registers)
    Ok(DriveTelemetry {
        protocol: Some(TelemetryProtocol::GenericBlock),
        temperature_c: None,
        power_on_hours: None,
        power_cycles: None,
        reallocated_sectors: Some(0),
        pending_sectors: Some(0),
        uncorrectable_sectors: Some(0),
        wear_percentage: Some(0),
        available_spare_percentage: Some(100),
        data_read_tb: None,
        data_written_tb: None,
        critical_warning: Some(0),
        max_safe_temp_c: Some(55),
        critical_temp_c: Some(75),
        attributes: Vec::new(),
    })
}

/// Lightweight thermal polling hook for the imager/safety watcher
#[allow(dead_code)]
pub fn poll_temperature(handle: HANDLE, disk_number: u32, bus_type: &BusType) -> Option<u32> {
    query_telemetry(handle, disk_number, bus_type)
        .ok()
        .and_then(|t| t.temperature_c)
}
