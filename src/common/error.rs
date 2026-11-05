use thiserror::Error;

#[derive(Error, Debug)]
#[allow(dead_code)]
pub enum AegisError {
    #[error("Administrative privileges required. Run as Administrator.")]
    ElevationRequired,

    #[error("Target drive {0} is the active Windows System/Boot disk. Destructive operations are strictly prohibited.")]
    SystemDiskProtected(u32),

    #[error("Device handle error for {0}: {1}")]
    DeviceOpenFailed(String, u32),

    #[error("IOCTL failed: {0} (Win32 OS error {1})")]
    IoctlFailed(String, u32),

    #[error("Memory alignment violation: buffer address 0x{0:X} not aligned to sector size {1}")]
    AlignmentViolation(usize, usize),

    #[error("Partition snapshot failed for drive {0}: {1}")]
    SnapshotFailed(u32, String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, AegisError>;
