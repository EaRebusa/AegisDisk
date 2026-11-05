# AegisDisk: Complete Vulnerability, Data Safety & Hardware Mitigation Specification

This document provides a comprehensive, finalized breakdown of hardware vulnerabilities, OS-level traps, data-loss failure modes, and architectural mitigations for AegisDisk on Windows and Linux.

---

## 1. Critical Data Loss & Safety Vulnerabilities

### 1.1 System & Boot Drive Accidental Overwrite
* **The Risk**: When executing raw sector writes or dismounts, mistaking `\\.\PhysicalDrive0` (or `/dev/nvme0n1` / `/dev/sda`) for an external target will instantly corrupt the Windows Boot Configuration Data (BCD), partition table, or active operating system files, rendering the machine unbootable.
* **Root Cause**: Reliance on raw disk index numbers (`PhysicalDriveX`) without validating system volume flags.
* **Mitigation Protocol**:
  1. Query `IOCTL_STORAGE_GET_DEVICE_NUMBER` and cross-reference with system volume paths (`GetSystemDirectoryW`, `GetWindowsDirectoryW`).
  2. Query `IOCTL_DISK_GET_DRIVE_LAYOUT_EX` to inspect partition styles. If any partition contains the active boot flag, system flag, or crash dump file, mark the drive as **IMMUTABLE_SYSTEM**.
  3. Hard-block destructive write modes on any drive flagged as `IMMUTABLE_SYSTEM`.
  4. Require two-factor user confirmation: user must manually type the drive's serial number and capacity string.

### 1.2 Destructive LBA Collisions & Partition Table Safeguard (Raw Restore Point)
* **The Risk**: SPEC.md proposed writing signature packets to $2^n$ boundary sectors. In real-world partition schemes, power-of-two LBAs collide with MBR, GPT arrays, and file system superblocks.
* **The "Restore Point" Solution**: Windows OS Restore Points (`Checkpoint-Computer`) only cover system registry/OS files; they **cannot** restore raw physical sectors or non-OS partitions.
* **AegisDisk Raw Metadata Snapshot**:
  1. Before performing any write operation or fake-capacity probe, AegisDisk must read and dump:
     * **LBA 0 to LBA 63**: Primary MBR, Protective MBR, and Primary GPT Header & Partition Array.
     * **Last 33 LBAs**: Backup Secondary GPT Header and Partition Array.
  2. Write this binary blob to an encrypted or timestamped backup file: `./backups/<DriveSerial>_LBA_SNAPSHOT.bin`.
  3. Provide an emergency one-command rollback (`aegisdisk restore-partition --disk X --backup-file ...`) to restore corrupted partition headers in seconds.

### 1.3 Imager Destination Collisions & Free Space Starvation
* **The Risk**: When salvaging data to a raw image (`.raw` / `.dd`), selecting a destination path residing on the failing drive itself, or selecting a volume with insufficient free space, causes an unrecoverable out-of-space halt mid-imaging or clobbers remaining readable sectors.
* **Mitigation Protocol**:
  1. Resolve the destination file's canonical volume path and verify its physical device number is strictly different from the source device number.
  2. Query `GetDiskFreeSpaceExW` to ensure destination available capacity exceeds `SourceDeviceTotalCapacity * 1.05` (5% safety margin for mapfile overhead).

### 1.4 Thermal Overheating & Hardware Death Spiral
* **The Risk**: Damaged drives with weak spindle bearings, degrading read heads, or failing NAND controllers rapidly overheat during intense multi-pass reads, causing catastrophic head crashes.
* **Mitigation Protocol**:
  1. In the background imaging loop, poll drive temperature via S.M.A.R.T. / NVMe Health Log every 30 seconds.
  2. If drive temperature exceeds a safe threshold (default 55°C for HDD, 70°C for NVMe), immediately pause reading and let the drive idle until temperature normalizes.

### 1.5 Windows Kernel Raw Write Lockouts (`ERROR_ACCESS_DENIED`)
* **The Risk**: Windows kernel storage drivers block raw sector writes to `\\.\PhysicalDriveX` if any file system on that disk is mounted.
* **Mitigation Protocol**:
  1. Enumerate all volume extents across the physical disk using `IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS`.
  2. For every volume found, open `\\.\<VolumeLetter>:` and issue `FSCTL_LOCK_VOLUME` and `FSCTL_DISMOUNT_VOLUME`.
  3. If dismount fails, prompt to place the disk offline via Windows VDS API (`IOCTL_DISK_SET_DISK_ATTRIBUTES` with `DISK_ATTRIBUTE_OFFLINE`).

### 1.6 Cache Spoofing on Fake Drive Probes
* **The Risk**: High-speed USB drives and fake SSDs return written data from RAM cache rather than physical flash, producing false passes.
* **Mitigation Protocol**:
  1. Open handles with `FILE_FLAG_NO_BUFFERING` and `FILE_FLAG_WRITE_THROUGH`.
  2. Call `FlushFileBuffers(handle)` immediately after test writes.
  3. Invalidate read buffers by interleaving writes across non-contiguous addresses.

---

## 2. Hardware & Operating System Edge Cases

### 2.1 Kernel Storport Freezes on Bad Sectors
* **The Risk**: Synchronous Win32 `ReadFile` hangs for 30–60 seconds per failing sector.
* **Mitigation Protocol**:
  1. Use Overlapped (Asynchronous) I/O (`FILE_FLAG_OVERLAPPED`).
  2. Enforce a strict watchdog timer (e.g., 500 ms) via `CancelIoEx`.
  3. On timeout, record the sector as damaged and jump ahead per multi-pass rescue rules.

### 2.2 USB Bridge Controller Dropouts & Resets
* **The Risk**: Failing USB-to-SATA/NVMe drives crash their bridge chip on bad blocks (`ERROR_DEVICE_NOT_CONNECTED`).
* **Mitigation Protocol**:
  1. Trap disconnection errors in the imager loop.
  2. Save mapfile state immediately to disk.
  3. Await re-enumeration via `RegisterDeviceNotificationW` matching device serial.
  4. Automatically re-acquire the handle and resume seamlessly.

### 2.3 USB Enclosure Protocol Discrepancies (UASP vs BOT vs SAT-16)
* **The Risk**: External enclosures fail standard NVMe pass-through commands (`IOCTL_STORAGE_PROTOCOL_COMMAND`).
* **Mitigation Protocol**:
  * 3-tier fallback query engine:
    1. Native `IOCTL_STORAGE_PROTOCOL_COMMAND`.
    2. SCSI Pass-Through SAT-16 (`IOCTL_SCSI_PASS_THROUGH`).
    3. Legacy ATA SMART (`SMART_RCV_DRIVE_DATA`).

### 2.4 Physical vs. Logical Sector Alignment (4Kn vs 512e)
* **The Risk**: Direct I/O requires buffer memory, length, and disk offset to be aligned to physical sector size, or Windows returns `ERROR_INVALID_PARAMETER` (87).
* **Mitigation Protocol**:
  1. Query `IOCTL_STORAGE_QUERY_PROPERTY` with `StorageAccessAlignmentProperty`.
  2. Use `aligned-vec` crate allocated to the reported hardware boundary.

### 2.5 SSD TRIM / Deallocate Zero-Fill Traps
* **The Risk**: Scanning deleted clusters on TRIMmed SSDs wastes hours scanning synthetic zero-fill blocks.
* **Mitigation Protocol**:
  1. Check device DZAT (Deterministic Read Zero after TRIM) capability.
  2. If consecutive cluster blocks yield pure zeros, trigger early exit with `TrimDiscardedZeroFill`.

---

## 3. Recommended Architectural & Toolchain Revisions

| Component | Initial Spec | Recommended Revision | Justification |
| :--- | :--- | :--- | :--- |
| **Partition Safeguard** | None | Raw LBA 0-63 + Backup GPT Snapshot (`.bin`) | Provides instant recovery if write tests or sector damage disrupts partitions. |
| **Thermal Monitor** | Telemetry display only | Active throttle / thermal pause in imager | Prevents physical drive death due to overheating. |
| **MFT Parser** | Hand-rolled raw binary parser | `ntfs` crate | Eliminates risk of corrupt file salvage from complex NTFS runlists. |
| **Direct I/O Buffers** | Custom pointer arithmetic | `aligned-vec` crate | Eliminates unsafe memory management; guarantees sector-aligned I/O. |
| **I/O Concurrency** | Ambiguous / Async loop | Dedicated worker thread + `crossbeam-channel` | Direct disk I/O on damaged hardware blocks threads; must be isolated from UI. |
| **Windows Elevation** | Runtime check | `build.rs` embedding UAC manifest | Enforces elevation on startup; avoids silent permission failures. |
| **Windows Win32 API**| `windows` or `windows-sys` | `windows-sys` | Faster compilation times and smaller binary size. |
