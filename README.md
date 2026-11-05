# AegisDisk 🛡️💾

[![License: MIT/Apache-2.0](https://img.shields.io/badge/License-MIT%2FApache--2.0-blue.svg)](LICENSE)
[![Platform: Windows x86_64](https://img.shields.io/badge/Platform-Windows%20x86__64-brightgreen.svg)]()
[![Rust Edition: 2024](https://img.shields.io/badge/Rust-2024%20Nightly%2FStable-orange.svg)]()

> **AegisDisk** is a bare-metal storage forensics, diagnostic, forensic disk-imaging, and raw file carving suite engineered in Rust for Windows systems. It bypasses OS caching and volume abstraction layers using direct Win32 sector I/O (`FILE_FLAG_NO_BUFFERING | FILE_FLAG_WRITE_THROUGH`), low-level ATA/NVMe/SCSI passthrough, and mathematical pattern engines to salvage data from degraded, failing, or counterfeit drives.

---

## 🌟 Key Subsystems & Architecture

```
                                  AegisDisk Architecture
┌────────────────────────────────────────────────────────────────────────────────────────┐
│                                 Ratatui TUI Interface                                  │
│  [F1: Dashboard]    [F2: Imager]    [F3: Fake Prober]    [F4: Surface Map]   [F5: Carver]  │
└───────────────────────────────────────────┬────────────────────────────────────────────┘
                                            │
┌───────────────────────────────────────────▼────────────────────────────────────────────┐
│                                      Aegis Core                                        │
│  ┌───────────────────┐ ┌───────────────────┐ ┌───────────────────┐ ┌──────────────────┐ │
│  │ Telemetry Engine  │ │ Non-Linear Imager │ │ Surface & Latency │ │ Signature Carver │ │
│  │ (NVMe/ATA/SAT)    │ │ (GNU ddrescue)    │ │ (PRNG & Watchdog) │ │ (33+ Filetypes)  │ │
│  └───────────────────┘ └───────────────────┘ └───────────────────┘ └──────────────────┘ │
└───────────────────────────────────────────┬────────────────────────────────────────────┘
                                            │
┌───────────────────────────────────────────▼────────────────────────────────────────────┐
│                             HAL (Hardware Abstraction Layer)                           │
│  Win32 Direct Aligned I/O (Sector-Granular) | SPTI (SCSI Pass-Through) | NVMe Passthrough│
└────────────────────────────────────────────────────────────────────────────────────────┘
```

---

## 🚀 Core Features

### 1. Low-Level Telemetry & Health Monitoring
* **Direct Bus Inquiry**: Communicates directly over `IOCTL_STORAGE_QUERY_PROPERTY` to extract genuine serial numbers, firmware revisions, physical sector sizes, and bus topologies (NVMe, SATA, USB, SCSI).
* **SMART Diagnostics**: Reads SMART attributes via ATA Pass-Through (`IOCTL_ATA_PASS_THROUGH`) and SCSI SAT (`0xA1`/`0x85` SCSI ATA Translation).
* **NVMe Health Log**: Queries NVMe Controller Health Log (`0x02`) for raw temperature composite thresholds, spare capacity warnings, and media error counts.
* **Thermal Throttling**: Real-time watchdog pauses destructive or high-stress workloads when controller temperatures exceed critical factory thresholds.

### 2. Multi-Pass Non-Linear Forensic Imager (`ddrescue`-style)
* **Multi-Phase Salvage Strategy**:
  1. **Phase 1 (Rapid Non-Scrape)**: Reads large contiguous blocks (e.g., 1 MB chunks), instantly skipping bad sectors to salvage healthy media before drive death.
  2. **Phase 2 (Trimming)**: Pinpoints bad block boundaries from both directions down to single 512-byte / 4096-byte sectors.
  3. **Phase 3 (Scraping)**: Repeatedly attempts single-sector extraction on remaining unreadable edges.
* **Resume & Mapfile Support**: Atomic human-readable mapfile checkpoints every completed domain, allowing power-loss resumption.
* **File System Aware Filtering**:
  * **NTFS Bitmap Filtering**: Parses `$Bitmap` cluster allocations to skip unallocated free space, accelerating cloning speeds by up to 10× on partially filled drives.
  * **FAT32 Multipart Splitting**: Automatically splits disk images into 4 GB spanning files (`.raw.001`, `.raw.002`, etc.) when saving to FAT32 recovery destinations.
  * **exFAT / ReFS / EXT4 Compatibility**: Preserves sector offset integrity across all modern volume layouts.

### 3. Sparse LBA Fake Storage Capacity Prober
* **Detects Counterfeit Flash & SSDs**: Identifies firmware-spoofed USB drives and SD cards that report fake multi-terabyte sizes.
* **Non-Destructive Backups**: Backs up all target sectors into `backups/` before running write verification probes.
* **Wraparound & Black-Hole Detection**:
  * Injects cryptographically seeded PRNG block patterns across geometric intervals.
  * Detects controller wrap-around (writes at 64 GB overwriting block 0).
  * Detects black-hole drives (firmware silently acknowledging writes but returning `0x00` or `0xFF` on read).
* **Auto-Restoration**: Automatically restores backed-up partition tables and original sector contents upon completion.

### 4. Direct Platter Surface & Latency Analyzer
* **Microsecond-Accurate Latency Mapping**: Measures raw read latencies per block to identify degrading weak magnetic or flash sectors before unrecoverable I/O errors occur.
* **Visual Density Map**: Renders an interactive 40-block platter surface view categorizing sector response times:
  * `<50ms` (Healthy)
  * `<150ms` (Acceptable)
  * `<500ms` (Warning / Sluggish)
  * `>500ms` (Critical / Pending Failure)
  * `Timeout / Unreadable` (Dead Sector)

### 5. High-Precision Signature File Carver
Direct-to-disk raw file carving engine running with strict sector alignment. Includes custom structural length calculation rulers (no artificial file size limits or truncated headers):

| Category | File Types Supported | Size Determination Strategy |
|:---|:---|:---|
| **Images** | `JPG`, `PNG`, `BMP`, `GIF`, `WEBP`, `TIFF`, `PCX`, `PSD` | Exact header lengths, dimension rulers, sub-block chunk walks, and marker scans. |
| **Documents** | `PDF`, `RTF`, `DOCX`, `XLSX`, `PPTX`, `TXT` | Trailer xref parsing, dictionary walks, ZIP central directory inspection, and ASCII entropy gating. |
| **Archives** | `ZIP`, `7Z`, `RAR`, `TAR`, `GZ`, `BZ2`, `WIM`, `ISO` | End-of-Central-Directory backward walks, LZMA/Brotli streams, and 48-bit bit-aligned EOS markers. |
| **Audio** | `MP3`, `WAV`, `FLAC`, `OGG`, `AU`, `WMA` | RIFF chunk structures, frame header bitrate equations, Vorbis comments, and ASF GUID properties. |
| **Video** | `MP4`, `MKV`, `AVI`, `MOV`, `FLV`, `MPG`, `WMV` | ISOBMFF atom trees (`moov`/`mdat`), Matroska EBML trees, RIFF AVI chunk trees, and MPEG pack streams. |
| **System** | `SQLITE`, `EXE` | SQLite B-tree page sizing equations, PE header optional directory sizes, and COFF tables. |

* **SSD TRIM / Deallocation Guard**: Detects contiguous runs of zero/fill blocks and accelerates scanning across empty SSD ranges without thrashing flash controllers.
* **OOXML Auto-Classifier**: Distinguishes between `.docx`, `.xlsx`, and `.pptx` by parsing internal MIME content types within carved ZIP archives.

---

## 🗄️ File System & Volume Support Matrix

| Filesystem | Partition & Structure Support | Recovery & Carving Capabilities |
|:---|:---|:---|
| **NTFS** | `$MFT` Record Parsing, `$Bitmap` Free Space Skip, Attribute Runlists | Direct Cluster Carving, Deleted MFT Record Scans, Metadata-Assisted Rescue |
| **exFAT** | Volume Boot Record, FAT Cluster Chains, Allocation Bitmaps | Full-range Direct I/O Carving, File Directory Entry Walking |
| **FAT32** | BPB, FAT Table, 4GB-bounded Spanning Image Output (`.001`, `.002`) | Unallocated Free-Space Salvage, Multipart Image Writing |
| **EXT4** | Superblock (`0xEF53`), Inode Table (256B), Extent Trees (`0xF30A`), Directory Blocks (`ext4_dir_entry_2`) | Direct Physical Extent Block Mapping, Deleted Inode Recovery (`dtime > 0`) |

---

## ⌨️ TUI Navigation & Keybindings

| Key | Context | Action |
|:---:|:---:|:---|
| `F1` | Global | Switch to **Dashboard** (Drive inventory, SMART telemetry, controller health) |
| `F2` | Global | Switch to **Non-Linear Imager** (Phase 1-3 cloning, mapfiles) |
| `F3` | Global | Switch to **Fake Storage Prober** (Destructive write-probe verification) |
| `F4` | Global | Switch to **Surface Map** (Interactive real-time latency inspector) |
| `F5` | Global | Switch to **Signature Carver** (Direct raw file extraction) |
| `Up` / `Down` | Table Views | Smooth scroll selected drives, files, or telemetry rows |
| `S` | Imager / Carver | Start or resume active scanning session |
| `P` | Imager / Carver | Pause / unpause running operation |
| `C` | Imager / Carver | Cancel active operation |
| `O` / `L` | Carver | Edit destination directory path |
| `T` | Carver | Set target file (`.raw` / `.img`) or type `live` for physical drive |
| `Q` / `Esc` | Global | Safely exit AegisDisk |

---

## 🛠️ Building & Installation

### Prerequisites
* **Windows 10 / 11 / Server 2016+ (x86_64)**
* **Rust (2024 Edition)**
* **Administrator Privileges**: Direct physical disk access (`\\.\PhysicalDriveX`) requires an elevated shell.

### Build from Source
```powershell
# Clone the repository
git clone https://github.com/EaRebusa/aegisdisk.git
cd aegisdisk

# Build an optimized release binary
cargo build --release

# Run unit and integration tests
cargo test

# Run EXT4 simulation test suite
cargo run --example simulate_ext4
```

### Running with Administrator Privileges
Launch Windows PowerShell or Windows Terminal as **Administrator**:
```powershell
.\target\release\aegisdisk.exe
```

---

## ⚠️ Safety Disclaimers

1. **Physical Drive Handles**: Operating on `\\.\PhysicalDriveX` bypasses all Windows filesystem safety buffers. Ensure you select the correct drive index before confirming operations.
2. **Fake Prober**: The `FakeCheck` feature is a write-verification test. While it backs up probe sectors before testing, it must **never** be run on drives with sensitive unbacked data.
3. **Hardware Failures**: If a drive exhibits physical head clicking or severe motor instability, power off immediately and consult a hardware cleanroom service.

---

## 📄 License
Licensed under either of:
* Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
* MIT license ([LICENSE-MIT](LICENSE-MIT))
at your option.
