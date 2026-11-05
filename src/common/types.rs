use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)]
pub enum BusType {
    Nvme,
    Sata,
    Usb,
    Scsi,
    Unknown(u32),
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct SectorAlignment {
    pub logical_sector_size: u32,
    pub physical_sector_size: u32,
}

impl Default for SectorAlignment {
    fn default() -> Self {
        Self {
            logical_sector_size: 512,
            physical_sector_size: 4096,
        }
    }
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct DiskGeometry {
    pub total_bytes: u64,
    pub total_sectors: u64,
    pub bytes_per_sector: u32,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct DiskInfo {
    pub disk_number: u32,
    pub device_path: String,
    pub model: String,
    pub serial_number: String,
    pub bus_type: BusType,
    pub is_system_boot: bool,
    pub alignment: SectorAlignment,
    pub geometry: DiskGeometry,
    pub used_bytes: Option<u64>,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct PartitionSnapshot {
    pub disk_number: u32,
    pub primary_headers: Vec<u8>,
    pub backup_gpt: Vec<u8>,
    pub snapshot_path: PathBuf,
}
