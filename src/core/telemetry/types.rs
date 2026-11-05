#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TelemetryProtocol {
    NativeNvme,
    AtaPassThrough,
    ScsiPassThroughSat,
    LegacyAtaSmart,
    GenericBlock,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct SmartAttribute {
    pub id: u8,
    pub name: String,
    pub current: u8,
    pub worst: u8,
    pub threshold: u8,
    pub raw_value: u64,
    pub is_failing: bool,
}

#[derive(Debug, Clone, Default)]
#[allow(dead_code)]
pub struct DriveTelemetry {
    pub protocol: Option<TelemetryProtocol>,
    pub temperature_c: Option<u32>,
    pub power_on_hours: Option<u64>,
    pub power_cycles: Option<u64>,
    pub reallocated_sectors: Option<u64>,
    pub pending_sectors: Option<u64>,
    pub uncorrectable_sectors: Option<u64>,
    pub wear_percentage: Option<u8>,
    pub available_spare_percentage: Option<u8>,
    pub data_read_tb: Option<f64>,
    pub data_written_tb: Option<f64>,
    pub critical_warning: Option<u8>,
    pub max_safe_temp_c: Option<u32>,
    pub critical_temp_c: Option<u32>,
    pub attributes: Vec<SmartAttribute>,
}
