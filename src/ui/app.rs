use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};

use crate::common::types::DiskInfo;
use crate::core::recovery::carver::CarvedFile;
use crate::core::surface::latency_map::{scan_single_block, LatencyBucket, SurfaceScanStats};
use crate::core::surface::probe::run_sparse_fake_probe;
use crate::core::telemetry::{query_telemetry, DriveTelemetry};
use crate::hal::buffer::AlignedBuffer;
use crate::hal::windows::{enumerate_disks, is_elevated, open_direct_io_handle};
use crate::ui::terminal::TuiTerminal;
use crate::ui::views::{
    carver_view::render_carver_view,
    dashboard::render_dashboard,
    imager_view::render_imager_view,
    modal::render_safety_modal,
    surface_view::render_surface_view,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CurrentView {
    Dashboard,
    SurfaceScan,
    Imager,
    Carver,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathEditTarget {
    Imager,
    Carver,
    CarverSource,
}

pub struct App {
    pub current_view: CurrentView,
    pub disks: Vec<DiskInfo>,
    pub selected_disk_index: usize,
    pub telemetry_cache: Option<DriveTelemetry>,
    pub is_elevated: bool,

    // Surface Scan State
    pub surface_blocks: Vec<LatencyBucket>,
    pub surface_stats: SurfaceScanStats,
    pub surface_paused: bool,
    pub scan_offset: u64,
    pub scan_start_time: Option<Instant>,

    // Carver State
    pub carved_files: Vec<CarvedFile>,
    pub selected_carve_index: usize,
    pub sample_hex: Vec<u8>,

    // Output Location State (for F4 Imager & F5 Carver)
    pub imager_dest_path: PathBuf,
    pub carver_dest_path: PathBuf,
    pub carver_source_file: Option<PathBuf>,
    pub is_editing_path: bool,
    pub path_input_buf: String,
    pub path_edit_target: Option<PathEditTarget>,

    // Imager State
    pub imager_bytes_copied: u64,
    pub imager_running: bool,
    pub imager_rx: Option<crossbeam_channel::Receiver<crate::core::imager::cloner::ImagerEvent>>,
    pub imager_status_msg: String,
    pub imager_mode: crate::core::imager::cloner::ImagingMode,
    pub imager_sparse: bool,

    // Carver Status
    pub carver_status_msg: String,
    pub carver_running: bool,
    pub carver_rx: Option<crossbeam_channel::Receiver<crate::core::recovery::carver::CarverEvent>>,
    pub carver_scanned_bytes: u64,
    pub carver_total_bytes: u64,
    pub carver_speed_mb_s: f64,
    pub carver_plaintext_gate: crate::core::recovery::carver::PlaintextGate,

    // Task Controls (Pause / Cancel)
    pub imager_paused: Arc<AtomicBool>,
    pub imager_cancelled: Arc<AtomicBool>,
    pub carver_paused: Arc<AtomicBool>,
    pub carver_cancelled: Arc<AtomicBool>,

    // Hotplug Watcher
    pub last_disk_scan: Instant,

    // Safety Gate Modal
    pub show_safety_modal: bool,
    pub modal_serial_input: String,
    pub modal_error: bool,
    #[allow(dead_code)]
    pub probe_status_msg: Option<String>,
}

impl App {
    pub fn new() -> Self {
        let elevated = is_elevated();
        let disks = enumerate_disks();
        let mut app = Self {
            current_view: CurrentView::Dashboard,
            disks,
            selected_disk_index: 0,
            telemetry_cache: None,
            is_elevated: elevated,

            surface_blocks: Vec::new(),
            surface_stats: SurfaceScanStats::default(),
            surface_paused: false,
            scan_offset: 0,
            scan_start_time: None,

            carved_files: Vec::new(),
            selected_carve_index: 0,
            sample_hex: vec![
                0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10, 0x4A, 0x46, 0x49, 0x46, 0x00, 0x01, 0x01, 0x01, 0x00, 0x60,
                0x00, 0x60, 0x00, 0x00, 0xFF, 0xDB, 0x00, 0x43, 0x00, 0x08, 0x06, 0x06, 0x07, 0x06, 0x05, 0x08,
            ],

            imager_dest_path: PathBuf::from("./backups"),
            carver_dest_path: PathBuf::from("./salvaged"),
            carver_source_file: None,
            is_editing_path: false,
            path_input_buf: String::new(),
            path_edit_target: None,

            imager_bytes_copied: 0,
            imager_running: false,
            imager_rx: None,
            imager_status_msg: String::new(),
            imager_mode: crate::core::imager::cloner::ImagingMode::SmartFilesystem,
            imager_sparse: true,
            carver_status_msg: String::new(),
            carver_running: false,
            carver_rx: None,
            carver_scanned_bytes: 0,
            carver_total_bytes: 0,
            carver_speed_mb_s: 0.0,
            carver_plaintext_gate: crate::core::recovery::carver::PlaintextGate::Off,
            imager_paused: Arc::new(AtomicBool::new(false)),
            imager_cancelled: Arc::new(AtomicBool::new(false)),
            carver_paused: Arc::new(AtomicBool::new(false)),
            carver_cancelled: Arc::new(AtomicBool::new(false)),
            last_disk_scan: Instant::now(),

            show_safety_modal: false,
            modal_serial_input: String::new(),
            modal_error: false,
            probe_status_msg: None,
        };

        app.refresh_selected_telemetry();
        app
    }

    pub fn refresh_selected_telemetry(&mut self) {
        if let Some(disk) = self.disks.get(self.selected_disk_index) {
            if let Ok(guard) = open_direct_io_handle(disk.disk_number, false) {
                self.telemetry_cache = query_telemetry(guard.0, disk.disk_number, &disk.bus_type).ok();
            } else {
                self.telemetry_cache = None;
            }
        }
    }

    pub fn run(&mut self, terminal: &mut TuiTerminal) -> anyhow::Result<()> {
        let chunk_size = 256 * 1024; // 256 KB
        let mut scan_buf = AlignedBuffer::new(chunk_size, 4096);

        loop {
            // Draw current view
            terminal.draw(|f| {
                let area = f.area();
                match self.current_view {
                    CurrentView::Dashboard => {
                        render_dashboard(
                            f,
                            area,
                            &self.disks,
                            self.selected_disk_index,
                            self.telemetry_cache.as_ref(),
                            self.is_elevated,
                        );
                    }
                    CurrentView::SurfaceScan => {
                        let disk = self.disks.get(self.selected_disk_index).cloned().unwrap_or_else(|| {
                            DiskInfo {
                                disk_number: 0,
                                device_path: String::new(),
                                model: "Unknown".to_string(),
                                serial_number: "Unknown".to_string(),
                                bus_type: crate::common::types::BusType::Unknown(0),
                                is_system_boot: false,
                                alignment: Default::default(),
                                geometry: crate::common::types::DiskGeometry {
                                    total_bytes: 100 * 1024 * 1024 * 1024,
                                    total_sectors: 0,
                                    bytes_per_sector: 512,
                                },
                                used_bytes: None,
                            }
                        });
                        let elapsed = self.scan_start_time.map(|t| t.elapsed().as_secs()).unwrap_or(0);
                        render_surface_view(
                            f,
                            area,
                            &disk,
                            &self.surface_blocks,
                            &self.surface_stats,
                            self.surface_paused,
                            elapsed,
                        );
                    }
                    CurrentView::Imager => {
                        let disk = self.disks.get(self.selected_disk_index).cloned().unwrap_or_else(|| {
                            DiskInfo {
                                disk_number: 0,
                                device_path: String::new(),
                                model: "Unknown".to_string(),
                                serial_number: "Unknown".to_string(),
                                bus_type: crate::common::types::BusType::Unknown(0),
                                is_system_boot: false,
                                alignment: Default::default(),
                                geometry: crate::common::types::DiskGeometry {
                                    total_bytes: 100 * 1024 * 1024 * 1024,
                                    total_sectors: 0,
                                    bytes_per_sector: 512,
                                },
                                used_bytes: None,
                            }
                        });
                        let is_editing = self.is_editing_path && self.path_edit_target == Some(PathEditTarget::Imager);
                        render_imager_view(
                            f,
                            area,
                            &disk,
                            &self.imager_dest_path,
                            is_editing,
                            &self.path_input_buf,
                            self.imager_running,
                            self.imager_bytes_copied,
                            &self.imager_status_msg,
                            self.imager_mode,
                            self.imager_sparse,
                        );
                    }
                    CurrentView::Carver => {
                        let is_editing_dest = self.is_editing_path && self.path_edit_target == Some(PathEditTarget::Carver);
                        let is_editing_source = self.is_editing_path && self.path_edit_target == Some(PathEditTarget::CarverSource);
                        let disk_num = self.disks.get(self.selected_disk_index).map(|d| d.disk_number).unwrap_or(0);
                        render_carver_view(
                            f,
                            area,
                            &self.carved_files,
                            self.selected_carve_index,
                            &self.sample_hex,
                            &self.carver_dest_path,
                            is_editing_dest,
                            is_editing_source,
                            &self.path_input_buf,
                            &self.carver_status_msg,
                            self.carver_running,
                            self.carver_scanned_bytes,
                            self.carver_total_bytes,
                            self.carver_speed_mb_s,
                            self.carver_source_file.as_deref(),
                            disk_num,
                            self.carver_plaintext_gate,
                        );
                    }
                }

                // If Safety Barrier Gate is active, render modal over top
                if self.show_safety_modal {
                    if let Some(disk) = self.disks.get(self.selected_disk_index) {
                        render_safety_modal(
                            f,
                            area,
                            disk,
                            &self.modal_serial_input,
                            self.modal_error,
                        );
                    }
                }
            })?;

            // Surface scanner progress tick: persistent across views if scan is active and not paused
            if !self.surface_paused && self.scan_start_time.is_some() {
                if let Some(disk) = self.disks.get(self.selected_disk_index) {
                    if self.scan_offset < disk.geometry.total_bytes {
                        if let Ok(guard) = open_direct_io_handle(disk.disk_number, false) {
                            for _ in 0..8 {
                                if self.scan_offset >= disk.geometry.total_bytes {
                                    self.surface_paused = true;
                                    break;
                                }
                                let res = scan_single_block(guard.0, self.scan_offset, &mut scan_buf);
                                self.surface_stats.record(res.bucket, chunk_size);
                                if self.surface_blocks.len() >= 2400 {
                                    self.surface_blocks.remove(0);
                                }
                                self.surface_blocks.push(res.bucket);
                                self.scan_offset += chunk_size as u64;
                            }
                        }
                    } else {
                        self.surface_paused = true;
                    }
                }
            }

            // Periodic live hotplug detection (every 2 seconds)
            if self.last_disk_scan.elapsed() >= Duration::from_secs(2) {
                self.last_disk_scan = Instant::now();
                let current_disks = enumerate_disks();
                let count_changed = current_disks.len() != self.disks.len();
                let ids_changed = current_disks
                    .iter()
                    .zip(&self.disks)
                    .any(|(a, b)| a.disk_number != b.disk_number || a.serial_number != b.serial_number);
                if count_changed || ids_changed {
                    self.disks = current_disks;
                    if self.selected_disk_index >= self.disks.len() {
                        self.selected_disk_index = self.disks.len().saturating_sub(1);
                    }
                    self.refresh_selected_telemetry();
                }
            }

            // Process async imager events
            if let Some(ref rx) = self.imager_rx {
                while let Ok(event) = rx.try_recv() {
                    match event {
                        crate::core::imager::cloner::ImagerEvent::Progress { rescued_bytes, speed_mb_s, .. } => {
                            self.imager_bytes_copied = rescued_bytes;
                            self.imager_status_msg = format!("Cloning: {:.1} MB/s", speed_mb_s);
                        }
                        crate::core::imager::cloner::ImagerEvent::Failed(err) => {
                            self.imager_running = false;
                            self.imager_status_msg = format!("Error: {}", err);
                        }
                        crate::core::imager::cloner::ImagerEvent::Completed { total_rescued, .. } => {
                            self.imager_running = false;
                            self.imager_bytes_copied = total_rescued;
                            self.imager_status_msg = "Completed successfully!".to_string();
                        }
                        _ => {}
                    }
                }
            }

            // Process async carver events
            if let Some(ref rx) = self.carver_rx {
                while let Ok(event) = rx.try_recv() {
                    match event {
                        crate::core::recovery::carver::CarverEvent::Started { total_bytes } => {
                            self.carver_total_bytes = total_bytes;
                            self.carver_status_msg = "Direct I/O scan active...".to_string();
                        }
                        crate::core::recovery::carver::CarverEvent::Progress {
                            scanned_bytes,
                            total_bytes,
                            speed_mb_s,
                            files_found: _,
                        } => {
                            self.carver_scanned_bytes = scanned_bytes;
                            self.carver_total_bytes = total_bytes;
                            self.carver_speed_mb_s = speed_mb_s;
                        }
                        crate::core::recovery::carver::CarverEvent::FileFound(cf) => {
                            self.carved_files.push(cf);
                        }
                        crate::core::recovery::carver::CarverEvent::Completed { total_found } => {
                            self.carver_running = false;
                            self.carver_status_msg = format!("Carve Completed: {} files salvaged", total_found);
                        }
                        crate::core::recovery::carver::CarverEvent::Failed(err) => {
                            self.carver_running = false;
                            self.carver_status_msg = format!("Carver Error: {}", err);
                        }
                    }
                }
            }

            // Input processing (poll 50ms)
            if event::poll(Duration::from_millis(50))? {
                match event::read()? {
                    Event::Paste(pasted) => {
                        let clean = pasted.trim_matches(|c| c == '\r' || c == '\n');
                        if self.is_editing_path {
                            self.path_input_buf.push_str(clean);
                        } else if self.show_safety_modal {
                            self.modal_serial_input.push_str(clean);
                        }
                    }
                    Event::Key(key) if key.kind == KeyEventKind::Press => {
                        if self.is_editing_path {
                            // Support Ctrl+V paste from Windows Clipboard
                            if key.modifiers.contains(KeyModifiers::CONTROL) && (key.code == KeyCode::Char('v') || key.code == KeyCode::Char('V')) {
                                if let Some(clip) = crate::hal::windows::get_clipboard_text() {
                                    let clean = clip.trim_matches(|c| c == '\r' || c == '\n');
                                    self.path_input_buf.push_str(clean);
                                }
                                continue;
                            }

                            // Support Ctrl+U or Ctrl+C to quickly clear or cancel
                            if key.modifiers.contains(KeyModifiers::CONTROL) && (key.code == KeyCode::Char('u') || key.code == KeyCode::Char('U')) {
                                self.path_input_buf.clear();
                                continue;
                            }
                            if key.modifiers.contains(KeyModifiers::CONTROL) && (key.code == KeyCode::Char('c') || key.code == KeyCode::Char('C')) {
                                self.is_editing_path = false;
                                self.path_input_buf.clear();
                                self.path_edit_target = None;
                                continue;
                            }

                            match key.code {
                                KeyCode::Esc => {
                                    self.is_editing_path = false;
                                    self.path_input_buf.clear();
                                    self.path_edit_target = None;
                                }
                                KeyCode::Backspace => {
                                    self.path_input_buf.pop();
                                }
                                KeyCode::Char(c) => {
                                    self.path_input_buf.push(c);
                                }
                                KeyCode::Enter => {
                                    let trimmed = self.path_input_buf.trim();
                                    if !trimmed.is_empty() {
                                        let new_path = PathBuf::from(trimmed);
                                        match self.path_edit_target {
                                            Some(PathEditTarget::Imager) => {
                                                let _ = std::fs::create_dir_all(&new_path);
                                                self.imager_dest_path = new_path;
                                            }
                                            Some(PathEditTarget::Carver) => {
                                                let _ = std::fs::create_dir_all(&new_path);
                                                self.carver_dest_path = new_path;
                                            }
                                            Some(PathEditTarget::CarverSource) => {
                                                let clean = trimmed.trim_matches(|c| c == '"' || c == '\'').trim();
                                                if clean.eq_ignore_ascii_case("live") || clean.eq_ignore_ascii_case("clear") {
                                                    self.carver_source_file = None;
                                                    self.carver_status_msg = "Carver target reset to Live PhysicalDrive.".to_string();
                                                } else {
                                                    let source_path = PathBuf::from(clean);
                                                    if source_path.is_dir() {
                                                        self.carver_status_msg = "Error: Folder specified! Target must be a .raw or .img file.".to_string();
                                                    } else if source_path.is_file() {
                                                        self.carver_source_file = Some(source_path);
                                                        self.carver_status_msg = "Carver target file configured.".to_string();
                                                    } else {
                                                        self.carver_status_msg = format!("File not found: {}", clean);
                                                    }
                                                }
                                            }
                                            None => {}
                                        }
                                    }
                                    self.is_editing_path = false;
                                    self.path_input_buf.clear();
                                    self.path_edit_target = None;
                                }
                                _ => {}
                            }
                            continue;
                        }

                        if self.show_safety_modal {
                            // Modal Input Handling
                            match key.code {
                                KeyCode::Esc => {
                                    self.show_safety_modal = false;
                                    self.modal_serial_input.clear();
                                    self.modal_error = false;
                                }
                                KeyCode::Backspace => {
                                    self.modal_serial_input.pop();
                                    self.modal_error = false;
                                }
                                KeyCode::Char(c) => {
                                    self.modal_serial_input.push(c);
                                    self.modal_error = false;
                                }
                                KeyCode::Enter => {
                                    if let Some(disk) = self.disks.get(self.selected_disk_index) {
                                        let clean_target = disk.serial_number.trim().to_lowercase();
                                        let clean_input = self.modal_serial_input.trim().to_lowercase();
                                        if clean_target == clean_input && !disk.is_system_boot {
                                            self.show_safety_modal = false;
                                            let backup_dir = PathBuf::from("./backups");
                                            let _ = run_sparse_fake_probe(disk, &backup_dir);
                                        } else {
                                            self.modal_error = true;
                                        }
                                    }
                                }
                                _ => {}
                            }
                            continue;
                        }

                        // Global Keys
                        match key.code {
                            KeyCode::Char('q') => {
                                if self.imager_running {
                                    self.imager_cancelled.store(true, Ordering::Relaxed);
                                }
                                if self.carver_running {
                                    self.carver_cancelled.store(true, Ordering::Relaxed);
                                }
                                return Ok(());
                            }
                            KeyCode::F(1) => {
                                let _ = terminal.clear();
                                self.current_view = CurrentView::Dashboard;
                            }
                            KeyCode::F(2) => {
                                let _ = terminal.clear();
                                self.current_view = CurrentView::SurfaceScan;
                                if self.scan_start_time.is_none() {
                                    self.scan_start_time = Some(Instant::now());
                                }
                            }
                            KeyCode::F(3) => {
                                if let Some(disk) = self.disks.get(self.selected_disk_index) {
                                    if !disk.is_system_boot {
                                        let _ = terminal.clear();
                                        self.show_safety_modal = true;
                                        self.modal_serial_input.clear();
                                        self.modal_error = false;
                                    }
                                }
                            }
                            KeyCode::F(4) => {
                                let _ = terminal.clear();
                                self.current_view = CurrentView::Imager;
                            }
                            KeyCode::F(5) => {
                                let _ = terminal.clear();
                                self.current_view = CurrentView::Carver;
                            }
                            KeyCode::Char('o') | KeyCode::Char('O') | KeyCode::Char('l') | KeyCode::Char('L') => {
                                match self.current_view {
                                    CurrentView::Imager => {
                                        self.is_editing_path = true;
                                        self.path_edit_target = Some(PathEditTarget::Imager);
                                        self.path_input_buf = self.imager_dest_path.to_string_lossy().to_string();
                                    }
                                    CurrentView::Carver => {
                                        self.is_editing_path = true;
                                        self.path_edit_target = Some(PathEditTarget::Carver);
                                        self.path_input_buf = self.carver_dest_path.to_string_lossy().to_string();
                                    }
                                    _ => {}
                                }
                            }
                            KeyCode::Char('t') | KeyCode::Char('T') => {
                                if self.current_view == CurrentView::Carver && !self.carver_running {
                                    self.is_editing_path = true;
                                    self.path_edit_target = Some(PathEditTarget::CarverSource);
                                    self.path_input_buf = self.carver_source_file.as_ref().map(|p| p.to_string_lossy().to_string()).unwrap_or_default();
                                }
                            }
                            KeyCode::Tab | KeyCode::Down | KeyCode::Char('j') => {
                                match self.current_view {
                                    CurrentView::Dashboard => {
                                        if !self.disks.is_empty() {
                                            let old_num = self.disks.get(self.selected_disk_index).map(|d| d.disk_number);
                                            self.selected_disk_index = (self.selected_disk_index + 1) % self.disks.len();
                                            let new_num = self.disks.get(self.selected_disk_index).map(|d| d.disk_number);
                                            if old_num != new_num {
                                                self.surface_blocks.clear();
                                                self.surface_stats = SurfaceScanStats::default();
                                                self.scan_offset = 0;
                                                self.scan_start_time = None;
                                                self.surface_paused = false;
                                            }
                                            self.refresh_selected_telemetry();
                                        }
                                    }
                                    CurrentView::Carver => {
                                        if !self.carved_files.is_empty() {
                                            self.selected_carve_index = (self.selected_carve_index + 1) % self.carved_files.len();
                                        }
                                    }
                                    _ => {}
                                }
                            }
                            KeyCode::Up | KeyCode::Char('k') => {
                                match self.current_view {
                                    CurrentView::Dashboard => {
                                        if !self.disks.is_empty() {
                                            let old_num = self.disks.get(self.selected_disk_index).map(|d| d.disk_number);
                                            self.selected_disk_index = if self.selected_disk_index == 0 {
                                                self.disks.len() - 1
                                            } else {
                                                self.selected_disk_index - 1
                                            };
                                            let new_num = self.disks.get(self.selected_disk_index).map(|d| d.disk_number);
                                            if old_num != new_num {
                                                self.surface_blocks.clear();
                                                self.surface_stats = SurfaceScanStats::default();
                                                self.scan_offset = 0;
                                                self.scan_start_time = None;
                                                self.surface_paused = false;
                                            }
                                            self.refresh_selected_telemetry();
                                        }
                                    }
                                    CurrentView::Carver => {
                                        if !self.carved_files.is_empty() {
                                            self.selected_carve_index = if self.selected_carve_index == 0 {
                                                self.carved_files.len() - 1
                                            } else {
                                                self.selected_carve_index - 1
                                            };
                                        }
                                    }
                                    _ => {}
                                }
                            }
                            KeyCode::Char(' ') => {
                                match self.current_view {
                                    CurrentView::SurfaceScan => {
                                        self.surface_paused = !self.surface_paused;
                                    }
                                    CurrentView::Imager => {
                                        if self.imager_running {
                                            let cur = self.imager_paused.load(Ordering::Relaxed);
                                            self.imager_paused.store(!cur, Ordering::Relaxed);
                                            self.imager_status_msg = if !cur {
                                                "Imaging Paused (press Space to resume)".to_string()
                                            } else {
                                                "Imaging Resumed".to_string()
                                            };
                                        }
                                    }
                                    CurrentView::Carver => {
                                        if self.carver_running {
                                            let cur = self.carver_paused.load(Ordering::Relaxed);
                                            self.carver_paused.store(!cur, Ordering::Relaxed);
                                            self.carver_status_msg = if !cur {
                                                "Carver Paused (press Space to resume)".to_string()
                                            } else {
                                                "Carver Resumed".to_string()
                                            };
                                        }
                                    }
                                    _ => {}
                                }
                            }
                            KeyCode::Char('x') | KeyCode::Char('X') => {
                                match self.current_view {
                                    CurrentView::Imager => {
                                        if self.imager_running {
                                            self.imager_cancelled.store(true, Ordering::Relaxed);
                                            self.imager_status_msg = "Stopping imaging session...".to_string();
                                        }
                                    }
                                    CurrentView::Carver => {
                                        if self.carver_running {
                                            self.carver_cancelled.store(true, Ordering::Relaxed);
                                            self.carver_status_msg = "Stopping carver scan...".to_string();
                                        }
                                    }
                                    _ => {}
                                }
                            }
                            KeyCode::Char('s') | KeyCode::Char('S') => {
                                if self.current_view == CurrentView::Carver && !self.carver_running {
                                    let is_ssd = self.disks.get(self.selected_disk_index).map(|d| matches!(d.bus_type, crate::common::types::BusType::Nvme)).unwrap_or(false);
                                    let carver = crate::core::recovery::carver::SignatureCarver::new_with_options(
                                        self.carver_dest_path.clone(),
                                        is_ssd,
                                        self.carver_plaintext_gate,
                                    );
                                    let (tx, rx) = crossbeam_channel::unbounded();
                                    self.carver_rx = Some(rx);
                                    self.carver_running = true;
                                    self.carver_paused.store(false, Ordering::Relaxed);
                                    self.carver_cancelled.store(false, Ordering::Relaxed);
                                    let pause_flag = Arc::clone(&self.carver_paused);
                                    let cancel_flag = Arc::clone(&self.carver_cancelled);

                                    if let Some(ref source_path) = self.carver_source_file {
                                        let source_path_clone = source_path.clone();
                                        let total_bytes = std::fs::metadata(&source_path_clone).map(|m| m.len()).unwrap_or(0);
                                        self.carver_scanned_bytes = 0;
                                        self.carver_total_bytes = total_bytes;
                                        self.carver_speed_mb_s = 0.0;
                                        self.carved_files.clear();
                                        self.carver_status_msg = format!("Carving from image: {}...", source_path_clone.file_name().unwrap_or_default().to_string_lossy());
                                        std::thread::spawn(move || {
                                            if let Err(e) = carver.carve_from_file(&source_path_clone, Some(tx.clone()), Some(pause_flag), Some(cancel_flag)) {
                                                let _ = tx.send(crate::core::recovery::carver::CarverEvent::Failed(e.to_string()));
                                            }
                                        });
                                    } else if let Some(disk) = self.disks.get(self.selected_disk_index) {
                                        let disk_number = disk.disk_number;
                                        let total_bytes = disk.geometry.total_bytes;
                                        let start_offset = if self.carver_scanned_bytes >= total_bytes {
                                            self.carved_files.clear();
                                            0
                                        } else {
                                            self.carver_scanned_bytes
                                        };
                                        self.carver_scanned_bytes = start_offset;
                                        self.carver_total_bytes = total_bytes;
                                        self.carver_speed_mb_s = 0.0;
                                        self.carver_status_msg = if start_offset > 0 {
                                            format!("Resuming Direct I/O Carver at offset 0x{:X}...", start_offset)
                                        } else {
                                            "Starting Direct I/O Carver...".to_string()
                                        };
                                        std::thread::spawn(move || {
                                            if let Err(e) = carver.carve_physical_drive(disk_number, start_offset, total_bytes, Some(tx.clone()), Some(pause_flag), Some(cancel_flag)) {
                                                let _ = tx.send(crate::core::recovery::carver::CarverEvent::Failed(e.to_string()));
                                            }
                                        });
                                    }
                                }
                            }
                            KeyCode::Char('u') | KeyCode::Char('U') => {
                                if self.current_view == CurrentView::Carver && !self.carver_running {
                                    self.carver_plaintext_gate = self.carver_plaintext_gate.next();
                                    self.carver_status_msg = format!("Plaintext Gate: {}", self.carver_plaintext_gate.label());
                                }
                            }
                            KeyCode::Char('m') | KeyCode::Char('M') => {
                                if self.current_view == CurrentView::Imager && !self.imager_running {
                                    self.imager_mode = match self.imager_mode {
                                        crate::core::imager::cloner::ImagingMode::ForensicRaw => {
                                            crate::core::imager::cloner::ImagingMode::SmartFilesystem
                                        }
                                        crate::core::imager::cloner::ImagingMode::SmartFilesystem => {
                                            crate::core::imager::cloner::ImagingMode::ForensicRaw
                                        }
                                    };
                                }
                            }
                            KeyCode::Char('p') | KeyCode::Char('P') => {
                                if self.current_view == CurrentView::Imager && !self.imager_running {
                                    let fs_kind = crate::hal::windows::query_filesystem_kind(&self.imager_dest_path);
                                    if !fs_kind.supports_sparse() && !self.imager_sparse {
                                        self.imager_status_msg = format!(
                                            "Sparse blocked: {} does not support sparse files (NTFS required)",
                                            fs_kind.as_str()
                                        );
                                    } else {
                                        self.imager_sparse = !self.imager_sparse;
                                        self.imager_status_msg = if self.imager_sparse {
                                            "Sparse destination enabled (NTFS zero compaction)".to_string()
                                        } else {
                                            "Sparse destination disabled (full preallocation)".to_string()
                                        };
                                    }
                                }
                            }
                            KeyCode::Char('i') | KeyCode::Char('I') => {
                                if self.current_view == CurrentView::Imager && !self.imager_running {
                                    if let Some(disk) = self.disks.get(self.selected_disk_index) {
                                        let _ = std::fs::create_dir_all(&self.imager_dest_path);
                                        let img_file = self.imager_dest_path.join(format!("PhysicalDrive{}_image.raw", disk.disk_number));
                                        let map_file = self.imager_dest_path.join(format!("PhysicalDrive{}_image.map", disk.disk_number));
                                        let mut config = crate::core::imager::cloner::ImagerConfig::default();
                                        config.mode = self.imager_mode;
                                        config.sparse_destination = self.imager_sparse;
                                        let (tx, rx) = crossbeam_channel::unbounded();
                                        self.imager_rx = Some(rx);
                                        self.imager_status_msg = "Initializing...".to_string();
                                        self.imager_running = true;
                                        self.imager_paused.store(false, Ordering::Relaxed);
                                        self.imager_cancelled.store(false, Ordering::Relaxed);
                                        let disk_clone = disk.clone();
                                        let pause_flag = Arc::clone(&self.imager_paused);
                                        let cancel_flag = Arc::clone(&self.imager_cancelled);
                                        std::thread::spawn(move || {
                                            crate::core::imager::cloner::run_imaging_session(disk_clone, img_file, map_file, config, tx, Some(pause_flag), Some(cancel_flag));
                                        });
                                    }
                                }
                            }
                            KeyCode::Esc => {
                                let _ = terminal.clear();
                                self.current_view = CurrentView::Dashboard;
                            }
                            _ => {}
                        }
                    }
                    _ => {}
                }
            }
        }
    }
}
