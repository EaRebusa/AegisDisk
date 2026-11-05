# AegisDisk: Safe Personal Drive Testing Protocol

This guide outlines exactly how to verify AegisDisk on your daily-driver personal disk safely, establishing zero-risk boundaries to preserve your files and partition tables.

---

## 1. Safety Matrix: What to Test vs. What to Avoid

| Subcommand / Action | Safe for Personal Drive? | Operation Mode | Impact / Consequence |
| :--- | :--- | :--- | :--- |
| `aegisdisk list` | **100% SAFE** | Read-Only | Queries device numbers, capacity, bus types, and flags boot partitions. Zero writes. |
| `aegisdisk health --disk <id>` | **100% SAFE** | Read-Only | Queries NVMe Health Log (0x02) or ATA SMART tables. Reads drive firmware registers without touching data sectors. |
| `aegisdisk snapshot --disk <id>` | **100% SAFE** | Read-Only | Reads LBA 0–63 (MBR/GPT) and backup GPT; dumps to `./backups/*.bin`. Completely passive. |
| `aegisdisk scan --disk <id>` | **100% SAFE** | Read-Only | Issues unbuffered `ReadFile` to benchmark latency. Does not alter a single bit. |
| `aegisdisk tui` (Views F1, F2) | **100% SAFE** | Read-Only | Interactive dashboard and live surface map. Passive monitoring only. |
| `aegisdisk fake-check` / `F3` | ⚠️ **DO NOT RUN ON PERSONAL DRIVE** | **DESTRUCTIVE WRITE** | Overwrites boundary sectors (1 GB, 2 GB, 4 GB...) with test signatures. Only run on blank test flash drives! |
| `aegisdisk restore-partition` | ⚠️ **DO NOT RUN** | **OVERWRITE WRITE** | Overwrites LBA 0–63 with a backup binary. Only use if a drive's partition table was damaged. |

---

## 2. Recommended Step-by-Step Verification on Personal Drive

### Step 1: Privilege & Device Discovery
Open PowerShell **as Administrator** and run:
```powershell
.\target\x86_64-pc-windows-gnullvm\debug\aegisdisk.exe list
```
* **What you should expect**:
  * Clean table listing every connected physical drive (`PhysicalDrive0`, `PhysicalDrive1`, etc.).
  * Model string and capacity in GB matching Windows Disk Management.
  * Your active Windows OS drive will display `[SYSTEM - PROTECTED]`.
  * Non-boot drives will display `[ACCESSIBLE]`.

### Step 2: Telemetry & Thermal Diagnostics
Identify your personal drive number (e.g., disk `0` or `1`) and run:
```powershell
.\target\x86_64-pc-windows-gnullvm\debug\aegisdisk.exe health --disk 0
```
* **What you should expect**:
  * Protocol source: `NativeNvme` (for internal M.2 SSDs) or `LegacyAtaSmart` / `ScsiPassThroughSat` (for SATA / USB).
  * Temperature readout: e.g., `41°C [NORMAL]`.
  * Health wear status: e.g., `Wear Level: 2% used` (or Available Spare `98%`).
  * If SATA HDD/SSD: a formatted table of all active S.M.A.R.T. raw attributes.

### Step 3: Zero-Risk Partition Snapshot (Precautionary Backup)
Create an instant binary backup of your drive's partition tables:
```powershell
.\target\x86_64-pc-windows-gnullvm\debug\aegisdisk.exe snapshot --disk 0 --out ./backups
```
* **What you should expect**:
  * AegisDisk reads LBA 0–63 (MBR + Primary GPT) and the secondary GPT table from the end of the drive.
  * Outputs: `[+] Partition snapshot saved successfully! -> ./backups/drive_0_<serial>_lba_snapshot.bin`.

### Step 4: Sample Surface Latency Scan (Read-Only)
Run a non-destructive sample scan over the first 20 blocks (5 MB):
```powershell
.\target\x86_64-pc-windows-gnullvm\debug\aegisdisk.exe scan --disk 0 --chunk-kb 256 --max-blocks 20
```
* **What you should expect**:
  * Live colored symbols printed per block:
    * Green `■` (<10ms): Fast flash response.
    * Light green `■` (<50ms): Normal flash/rotational response.
  * A final summary table reporting Fast, Normal, Sluggish, Critical, and Bad block counts. Zero writes are made to your media.

### Step 5: Launch the Interactive TUI
Launch the full terminal interface:
```powershell
.\target\x86_64-pc-windows-gnullvm\debug\aegisdisk.exe tui
```
* **How to navigate safely**:
  * `Tab` / `Up` / `Down`: Move between connected drives in the list.
  * `F1`: Main Dashboard (view live telemetry, life gauge, temp meter).
  * `F2`: Live Surface Map (press `Space` to pause or resume read scanning).
  * `Esc`: Return to Dashboard.
  * `Q`: Quit back to terminal cleanly.
  * **Safety Barrier Gate Test**: Press `F3`. A red overlay modal will appear. It is impossible to proceed without typing the exact serial number. Press `Esc` to dismiss it safely.

---

## 3. The 3 Golden Safety Rules for Your Personal Media
1. **Never pass `--confirm-serial` on your personal drive**: The `fake-check` tool writes test packets to sectors. Only use this command on suspicious, blank USB flash drives purchased online to test for fake capacity.
2. **Never execute raw restore without need**: `restore-partition` should only ever be used if partition headers were accidentally corrupted.
3. **Always check the `[SYSTEM - PROTECTED]` tag**: AegisDisk automatically hard-blocks raw write handles on any drive flagged as the Windows OS boot device.
