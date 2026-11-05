use std::io::Write;
use std::path::PathBuf;
use clap::{Parser, Subcommand};
use crossbeam_channel::unbounded;

mod common;
mod core;
mod hal;
mod ui;

#[derive(Parser)]
#[command(name = "aegisdisk")]
#[command(about = "AegisDisk: Storage Health, Triage & Recovery CLI/TUI", version = "0.1.0")]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Launch the interactive Ratatui TUI dashboard
    Tui,

    /// List all physical disks, bus types, and safety status
    List,

    /// Query hardware telemetry, wear status, temperature, and SMART logs
    Health {
        #[arg(short, long)]
        disk: u32,
    },

    /// Non-destructive surface latency benchmarking map
    Scan {
        #[arg(short, long)]
        disk: u32,
        #[arg(short, long, default_value_t = 256)]
        chunk_kb: usize,
        #[arg(short, long, default_value_t = 100)]
        max_blocks: u64,
    },

    /// Detect firmware-spoofed fake storage capacity (destructive write-probe)
    FakeCheck {
        #[arg(short, long)]
        disk: u32,
        #[arg(long)]
        confirm_serial: String,
        #[arg(short, long, default_value = "./backups")]
        backup_dir: PathBuf,
    },

    /// Salvage failing drive data into raw disk image (.raw/.img) with multi-pass rescue
    Image {
        #[arg(short, long)]
        disk: u32,
        #[arg(short, long)]
        out: PathBuf,
        #[arg(short, long)]
        mapfile: Option<PathBuf>,
        #[arg(long, default_value_t = 500)]
        timeout_ms: u32,
        #[arg(long, default_value_t = 55)]
        max_temp: u32,
    },

    /// Recover deleted files via magic signature carving with SSD TRIM awareness
    Carve {
        #[arg(short, long)]
        target: PathBuf,
        #[arg(short, long, default_value = "./recovered")]
        out: PathBuf,
        #[arg(long)]
        ssd: bool,
    },

    /// Create raw LBA 0-63 and backup GPT snapshot (.bin)
    Snapshot {
        #[arg(short, long)]
        disk: u32,
        #[arg(short, long, default_value = "./backups")]
        out: PathBuf,
    },

    /// Emergency rollback of partition headers from a snapshot file
    RestorePartition {
        #[arg(short, long)]
        disk: u32,
        #[arg(short, long)]
        file: PathBuf,
    },
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        None | Some(Commands::Tui) => {
            // Isolate tracing output into aegisdisk.log so stdout is never corrupted during TUI rendering
            if let Ok(log_file) = std::fs::File::create("aegisdisk.log") {
                let _ = tracing_subscriber::fmt()
                    .with_writer(log_file)
                    .with_ansi(false)
                    .try_init();
            }

            // Launch interactive Ratatui TUI
            let mut terminal = ui::terminal::setup_terminal()?;
            let mut app = ui::app::App::new();
            let res = app.run(&mut terminal);
            ui::terminal::restore_terminal(&mut terminal)?;
            res
        }
        Some(cmd) => {
            tracing_subscriber::fmt::init();
            execute_cli_command(cmd)
        }
    }
}

fn execute_cli_command(cmd: Commands) -> anyhow::Result<()> {
    match cmd {
        Commands::Tui => unreachable!(),
        Commands::List => {
            #[cfg(target_os = "windows")]
            {
                if !hal::windows::is_elevated() {
                    eprintln!("[!] WARNING: AegisDisk is not running with Administrator privileges.");
                    eprintln!("[!] Direct disk handles and hardware IOCTLs will fail.\n");
                }

                let disks = hal::windows::enumerate_disks();
                println!("{:<6} {:<24} {:<12} {:<14} {:<10} {:<18}", "DISK", "MODEL", "BUS", "SIZE (GB)", "SECTOR", "STATUS");
                println!("{:-<90}", "");

                for d in disks {
                    let gb = d.geometry.total_bytes as f64 / (1024.0 * 1024.0 * 1024.0);
                    let bus = format!("{:?}", d.bus_type);
                    let sector = format!("{}/{}", d.alignment.logical_sector_size, d.alignment.physical_sector_size);
                    let status = if d.is_system_boot {
                        "[SYSTEM - PROTECTED]"
                    } else {
                        "[ACCESSIBLE]"
                    };

                    println!(
                        "{:<6} {:<24} {:<12} {:<14.2} {:<10} {:<18}",
                        format!("PhysicalDrive{}", d.disk_number),
                        if d.model.len() > 22 { &d.model[..22] } else { &d.model },
                        bus,
                        gb,
                        sector,
                        status
                    );
                }
            }
            Ok(())
        }
        Commands::Health { disk } => {
            #[cfg(target_os = "windows")]
            {
                let disks = hal::windows::enumerate_disks();
                let disk_info = disks.iter().find(|d| d.disk_number == disk);

                let bus = disk_info.map(|d| d.bus_type.clone()).unwrap_or(common::types::BusType::Unknown(0));
                let model = disk_info.map(|d| d.model.as_str()).unwrap_or("Unknown");
                let serial = disk_info.map(|d| d.serial_number.as_str()).unwrap_or("Unknown");

                println!("============================================================");
                println!(" AegisDisk Telemetry: PhysicalDrive{} ({})", disk, model);
                println!(" Serial: {} | Bus: {:?}", serial, bus);
                println!("============================================================");

                let handle_res = hal::windows::open_direct_io_handle(disk, false);
                match handle_res {
                    Ok(handle_guard) => {
                        match core::telemetry::query_telemetry(handle_guard.0, disk, &bus) {
                            Ok(t) => {
                                println!(" Protocol Source: {:?}", t.protocol.unwrap_or(core::telemetry::TelemetryProtocol::NativeNvme));

                                let temp_str = match t.temperature_c {
                                    Some(temp) if temp >= 70 => format!("{}°C [CRITICAL OVERHEAT]", temp),
                                    Some(temp) if temp >= 55 => format!("{}°C [THERMAL WARNING]", temp),
                                    Some(temp) => format!("{}°C [NORMAL]", temp),
                                    None => "N/A".to_string(),
                                };
                                println!(" Temperature:     {}", temp_str);

                                if let Some(wear) = t.wear_percentage {
                                    println!(" Wear Level:      {}% used", wear);
                                }
                                if let Some(spare) = t.available_spare_percentage {
                                    println!(" Available Spare: {}%", spare);
                                }
                                if let Some(realloc) = t.reallocated_sectors {
                                    println!(" Reallocated Sec: {}", realloc);
                                }
                                if let Some(pending) = t.pending_sectors {
                                    println!(" Pending Sectors: {}", pending);
                                }
                                if let Some(uncorr) = t.uncorrectable_sectors {
                                    println!(" Uncorrectable:   {}", uncorr);
                                }
                                if let Some(poh) = t.power_on_hours {
                                    println!(" Power-On Hours:  {} hrs ({:.1} days)", poh, poh as f64 / 24.0);
                                }
                                if let Some(pc) = t.power_cycles {
                                    println!(" Power Cycles:    {}", pc);
                                }
                                if let (Some(r), Some(w)) = (t.data_read_tb, t.data_written_tb) {
                                    println!(" Host Read:       {:.2} TB | Host Written: {:.2} TB", r, w);
                                }

                                if !t.attributes.is_empty() {
                                    println!("\n{:<4} {:<28} {:<8} {:<8} {:<16}", "ID", "ATTRIBUTE", "CURRENT", "WORST", "RAW DATA");
                                    println!("{:-<70}", "");
                                    for a in t.attributes {
                                        println!("{:<4} {:<28} {:<8} {:<8} {:<16}", format!("0x{:02X}", a.id), a.name, a.current, a.worst, a.raw_value);
                                    }
                                }
                            }
                            Err(e) => eprintln!("[-] Failed querying telemetry: {}", e),
                        }
                    }
                    Err(e) => eprintln!("[-] Failed opening device handle: {}", e),
                }
            }
            Ok(())
        }
        Commands::Scan { disk, chunk_kb, max_blocks } => {
            #[cfg(target_os = "windows")]
            {
                let disks = hal::windows::enumerate_disks();
                let disk_info = disks.iter().find(|d| d.disk_number == disk);
                if disk_info.is_none() {
                    eprintln!("[-] Drive PhysicalDrive{} not found.", disk);
                    return Ok(());
                }
                let info = disk_info.unwrap();

                let chunk_bytes = chunk_kb * 1024;
                let mut buffer = hal::buffer::AlignedBuffer::new(chunk_bytes, info.alignment.physical_sector_size as usize);

                println!("============================================================");
                println!(" AegisDisk Surface Scan: PhysicalDrive{} ({} KB/block)", disk, chunk_kb);
                println!(" Legend: \x1b[32m■ <10ms\x1b[0m \x1b[92m■ <50ms\x1b[0m \x1b[33m■ <150ms\x1b[0m \x1b[38;5;208m■ <500ms\x1b[0m \x1b[31m■ >500ms\x1b[0m \x1b[35mX Error\x1b[0m");
                println!("============================================================");

                let handle_guard = hal::windows::open_direct_io_handle(disk, false)?;
                let handle = handle_guard.0;

                let mut stats = core::surface::latency_map::SurfaceScanStats::default();
                let limit = max_blocks.min(info.geometry.total_bytes / chunk_bytes as u64);

                for b in 0..limit {
                    let offset = b * chunk_bytes as u64;
                    let result = core::surface::latency_map::scan_single_block(handle, offset, &mut buffer);
                    stats.record(result.bucket, chunk_bytes);

                    print!("{}{}\x1b[0m", result.bucket.color_code(), result.bucket.symbol());
                    if (b + 1) % 50 == 0 {
                        println!(" [{:.1} MB]", (offset + chunk_bytes as u64) as f64 / (1024.0 * 1024.0));
                    }
                }

                println!("\n\n--- Scan Results ---");
                println!(" Total Blocks: {} ({:.2} MB)", stats.total_blocks, stats.total_bytes_scanned as f64 / (1024.0 * 1024.0));
                println!(" Fast (<10ms):     {}", stats.fast_count);
                println!(" Normal (<50ms):   {}", stats.normal_count);
                println!(" Sluggish (<150ms):{}", stats.sluggish_count);
                println!(" Critical (<500ms):{}", stats.critical_count);
                println!(" Timeout (>500ms): {}", stats.timeout_count);
                println!(" Read Errors:      {}", stats.error_count);
            }
            Ok(())
        }
        Commands::FakeCheck { disk, confirm_serial, backup_dir } => {
            #[cfg(target_os = "windows")]
            {
                let disks = hal::windows::enumerate_disks();
                let disk_info = disks.iter().find(|d| d.disk_number == disk);
                if disk_info.is_none() {
                    eprintln!("[-] Drive PhysicalDrive{} not found.", disk);
                    return Ok(());
                }
                let info = disk_info.unwrap();

                if info.is_system_boot {
                    eprintln!("[!] SAFETY HALT: PhysicalDrive{} is the Windows System/Boot Drive!", disk);
                    eprintln!("[!] Fake capacity write probes are strictly prohibited on system drives.");
                    return Ok(());
                }

                let clean_info_serial = info.serial_number.trim().to_lowercase();
                let clean_input_serial = confirm_serial.trim().to_lowercase();

                if clean_info_serial != clean_input_serial {
                    eprintln!("[!] SAFETY GATE: Serial number confirmation mismatch!");
                    eprintln!("    Target Drive Serial: '{}'", info.serial_number);
                    eprintln!("    User Provided:       '{}'", confirm_serial);
                    eprintln!("    To proceed, re-run with: --confirm-serial \"{}\"", info.serial_number.trim());
                    return Ok(());
                }

                println!("[*] Pre-execution safety check passed.");
                println!("[*] Starting Sparse LBA Probe on PhysicalDrive{}...", disk);

                match core::surface::probe::run_sparse_fake_probe(info, &backup_dir) {
                    Ok(outcome) => {
                        println!("============================================================");
                        if outcome.is_fake {
                            println!(" [!] WARNING: FAKE DRIVE DETECTED! [!]");
                            println!(" Declared Firmware Capacity: {:.2} GB", outcome.declared_capacity_bytes as f64 / 1e9);
                            println!(" Verified Physical Capacity: {:.2} GB", outcome.verified_capacity_bytes as f64 / 1e9);
                            println!(" Anomaly Detected:          {}", outcome.anomaly.unwrap_or_default());
                        } else {
                            println!(" [+] GENUINE DRIVE VERIFIED");
                            println!(" Declared Firmware Capacity: {:.2} GB", outcome.declared_capacity_bytes as f64 / 1e9);
                            println!(" Verified Physical Capacity: {:.2} GB", outcome.verified_capacity_bytes as f64 / 1e9);
                            println!(" No wrap-around or black-hole drops found.");
                        }
                        println!("============================================================");
                    }
                    Err(e) => eprintln!("[-] Probe failed: {}", e),
                }
            }
            Ok(())
        }
        Commands::Image { disk, out, mapfile, timeout_ms, max_temp } => {
            #[cfg(target_os = "windows")]
            {
                let disks = hal::windows::enumerate_disks();
                let disk_info = disks.into_iter().find(|d| d.disk_number == disk);
                if disk_info.is_none() {
                    eprintln!("[-] Drive PhysicalDrive{} not found.", disk);
                    return Ok(());
                }
                let info = disk_info.unwrap();

                let map_path = mapfile.unwrap_or_else(|| {
                    let mut p = out.clone();
                    p.set_extension("map");
                    p
                });

                let config = core::imager::cloner::ImagerConfig {
                    read_timeout_ms: timeout_ms,
                    max_temp_c: max_temp,
                    chunk_size: 64 * 1024,
                    skip_size_on_error: 1024 * 1024,
                    mode: core::imager::cloner::ImagingMode::SmartFilesystem,
                    sparse_destination: true,
                };

                println!("============================================================");
                println!(" AegisDisk Raw Drive Imager: PhysicalDrive{} -> {}", disk, out.display());
                println!(" Mapfile: {}", map_path.display());
                println!(" Timeout: {} ms | Max Temp: {}°C", timeout_ms, max_temp);
                println!("============================================================");

                let (tx, rx) = unbounded();
                let cloner_handle = std::thread::spawn(move || {
                    core::imager::cloner::run_imaging_session(info, out, map_path, config, tx, None, None);
                });

                for event in rx {
                    match event {
                        core::imager::cloner::ImagerEvent::Started { total_bytes, mode, sparse } => {
                            println!("[+] Imaging initialized. Total Capacity: {:.2} GB | Mode: {:?} | Sparse: {}", total_bytes as f64 / 1e9, mode, sparse);
                        }
                        core::imager::cloner::ImagerEvent::Progress {
                            pos,
                            rescued_bytes,
                            bad_bytes,
                            pending_bytes,
                            speed_mb_s,
                            pass,
                            ..
                        } => {
                            print!(
                                "\r[Pass {}] Pos: {:.1} GB | Rescued: {:.1} GB | Bad: {:.1} MB | Left: {:.1} GB | Rate: {:.2} MB/s   ",
                                pass,
                                pos as f64 / 1e9,
                                rescued_bytes as f64 / 1e9,
                                bad_bytes as f64 / (1024.0 * 1024.0),
                                pending_bytes as f64 / 1e9,
                                speed_mb_s
                            );
                            let _ = std::io::stdout().flush();
                        }
                        core::imager::cloner::ImagerEvent::ThermalPause { temp_c, threshold_c } => {
                            println!("\n[!] THERMAL THROTTLE: Drive temp {}°C reached limit {}°C. Pausing for cooldown...", temp_c, threshold_c);
                        }
                        core::imager::cloner::ImagerEvent::ThermalResumed { temp_c } => {
                            println!("[+] Drive cooled down to {}°C. Resuming clone...", temp_c);
                        }
                        core::imager::cloner::ImagerEvent::DeviceDisconnected => {
                            println!("\n[!] USB CONTROLLER RESET: Device disconnected! Mapfile saved. Awaiting reconnection...");
                        }
                        core::imager::cloner::ImagerEvent::DeviceReconnected => {
                            println!("[+] Device re-enumerated. Re-acquiring handle and resuming seamlessly...");
                        }
                        core::imager::cloner::ImagerEvent::Completed { total_rescued, total_bad } => {
                            println!("\n\n[+] IMAGING COMPLETE!");
                            println!("    Rescued Data: {:.2} GB", total_rescued as f64 / 1e9);
                            println!("    Dead Sectors: {:.2} MB", total_bad as f64 / (1024.0 * 1024.0));
                        }
                        core::imager::cloner::ImagerEvent::Failed(err) => {
                            eprintln!("\n[-] Imaging aborted: {}", err);
                        }
                    }
                }

                let _ = cloner_handle.join();
            }
            Ok(())
        }
        Commands::Carve { target, out, ssd } => {
            println!("============================================================");
            println!(" AegisDisk File Carver: {} -> {}", target.display(), out.display());
            println!(" Media Mode: {}", if ssd { "SSD (TRIM zero-bail enabled)" } else { "Rotational HDD" });
            println!(" File Signatures: JPEG, PNG, PDF, ZIP/DOCX");
            println!("============================================================");

            let carver = core::recovery::carver::SignatureCarver::new(out, ssd);
            let target_str = target.to_string_lossy().to_string();
            let res = if target_str.to_lowercase().starts_with(r"\\.\physicaldrive") {
                let disk_num_str = target_str.chars().filter(|c| c.is_ascii_digit()).collect::<String>();
                let disk_num = disk_num_str.parse::<u32>().unwrap_or(0);
                let disks = hal::windows::enumerate_disks();
                let total_bytes = disks.iter().find(|d| d.disk_number == disk_num).map(|d| d.geometry.total_bytes).unwrap_or(1024 * 1024 * 1024 * 1024);
                carver.carve_physical_drive(disk_num, 0, total_bytes, None, None, None)
            } else {
                carver.carve_from_file(&target, None, None, None)
            };
            match res {
                Ok(files) => {
                    println!("\n[+] CARVING COMPLETE: {} files salvaged!", files.len());
                    for f in files {
                        println!("    [{:?}] Offset: 0x{:08X} ({:.2} KB) -> {}", f.file_type, f.offset, f.size_bytes as f64 / 1024.0, f.output_path.display());
                    }
                }
                Err(e) => eprintln!("[-] Carving failed: {}", e),
            }
            Ok(())
        }
        Commands::Snapshot { disk, out } => {
            #[cfg(target_os = "windows")]
            {
                println!("[*] Creating partition snapshot for PhysicalDrive{}...", disk);
                match hal::windows::create_partition_snapshot(disk, &out) {
                    Ok(snapshot) => {
                        println!("[+] Partition snapshot saved successfully!");
                        println!("    Snapshot Path: {}", snapshot.snapshot_path.display());
                        println!("    Primary Headers: {} bytes", snapshot.primary_headers.len());
                        println!("    Backup GPT: {} bytes", snapshot.backup_gpt.len());
                    }
                    Err(e) => {
                        eprintln!("[-] Snapshot failed: {}", e);
                    }
                }
            }
            Ok(())
        }
        Commands::RestorePartition { disk, file } => {
            #[cfg(target_os = "windows")]
            {
                println!("[!] RESTORE WARNING: Overwriting LBA 0-63 on PhysicalDrive{}!", disk);
                match hal::windows::restore_partition_snapshot(disk, &file) {
                    Ok(()) => {
                        println!("[+] Partition headers restored successfully from {}", file.display());
                    }
                    Err(e) => {
                        eprintln!("[-] Restore failed: {}", e);
                    }
                }
            }
            Ok(())
        }
    }
}
