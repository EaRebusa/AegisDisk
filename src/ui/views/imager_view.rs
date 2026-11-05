use std::path::Path;
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Gauge, Paragraph},
    Frame,
};

use crate::common::types::DiskInfo;
use crate::core::imager::cloner::ImagingMode;

pub fn render_imager_view(
    f: &mut Frame,
    area: Rect,
    disk: &DiskInfo,
    dest_dir: &Path,
    is_editing_path: bool,
    path_input: &str,
    imaging_in_progress: bool,
    bytes_copied: u64,
    status_msg: &str,
    mode: ImagingMode,
    sparse_destination: bool,
) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // Top Header
            Constraint::Length(8), // Source & Destination paths + Mode + FS tag
            Constraint::Min(8),    // Imaging Stages & Progress Gauges
            Constraint::Length(3), // Action / status bar
        ])
        .split(area);

    // 1. Header
    let header_para = Paragraph::new(format!(
        " Resilient Win32 Overlapped Direct I/O Imager | Target: PhysicalDrive{} ({})",
        disk.disk_number, disk.model
    ))
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title(" AegisDisk - Resilient Raw & Smart Imager "),
    );
    f.render_widget(header_para, chunks[0]);

    // 2. Source & Destination Panel
    let gb_total = disk.geometry.total_bytes as f64 / 1e9;
    let image_filename = format!("PhysicalDrive{}_image.raw", disk.disk_number);
    let map_filename = format!("PhysicalDrive{}_image.map", disk.disk_number);
    let full_image_path = dest_dir.join(&image_filename);
    let full_map_path = dest_dir.join(&map_filename);

    let fs_kind = crate::hal::windows::query_filesystem_kind(dest_dir);
    let fs_tag = match fs_kind {
        crate::hal::windows::FileSystemKind::Ntfs => "NTFS (Sparse Ready)",
        crate::hal::windows::FileSystemKind::ExFat => "exFAT (Physical Preallocation)",
        crate::hal::windows::FileSystemKind::Fat32 => "FAT32 (Auto 3.9GB Split)",
        crate::hal::windows::FileSystemKind::Other => "Unknown Filesystem",
    };

    let mode_str = match mode {
        ImagingMode::ForensicRaw => "Mode 1: Forensic Full (Bit-by-Bit DD + Slack Space)",
        ImagingMode::SmartFilesystem => "Mode 2: Smart Filesystem (NTFS $Bitmap Skip Free Clusters)",
    };
    let sparse_str = if sparse_destination { "ON (NTFS physical zero compaction)" } else { "OFF (Preallocated space)" };

    let location_text = if is_editing_path {
        format!(
            " > ENTER NEW OUTPUT DIRECTORY: {}\n [Press ENTER to Confirm, ESC to Cancel]",
            path_input
        )
    } else {
        format!(
            " Source Drive: PhysicalDrive{} ({:.2} GB) [{}]\n Target Image:  {}\n Rescue Mapfile: {}\n Target FS:     {}\n Imager Mode:   {} [Press 'M' to switch]\n Sparse File:   {} [Press 'P' to toggle]",
            disk.disk_number,
            gb_total,
            if disk.is_system_boot { "SYSTEM PROTECTED" } else { "ACCESSIBLE" },
            full_image_path.to_string_lossy(),
            full_map_path.to_string_lossy(),
            fs_tag,
            mode_str,
            sparse_str,
        )
    };

    let loc_style = if is_editing_path {
        Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    };

    let loc_para = Paragraph::new(location_text)
        .style(loc_style)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(if is_editing_path { " EDITING OUTPUT PATH " } else { " Target File Destinations & Configuration " }),
        );
    f.render_widget(loc_para, chunks[1]);

    // 3. Progress & Stages
    let progress_pct = if disk.geometry.total_bytes > 0 {
        ((bytes_copied as f64 / disk.geometry.total_bytes as f64) * 100.0).clamp(0.0, 100.0)
    } else {
        0.0
    };

    let stage_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // Progress Gauge
            Constraint::Min(4),    // Pass details
        ])
        .split(chunks[2]);

    let gauge = Gauge::default()
        .block(Block::default().title(format!(" Total Cloned: {:.2} GB / {:.2} GB ({:.1}%) ", bytes_copied as f64 / 1e9, gb_total, progress_pct)))
        .gauge_style(Style::default().fg(Color::Cyan))
        .percent(progress_pct as u16);
    f.render_widget(gauge, stage_chunks[0]);

    let stage_info = vec![
        Line::from(vec![
            Span::styled(" [Phase 1: Sequential Copy] ", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
            Span::raw("High-speed 64KB Direct I/O streaming across healthy regions."),
        ]),
        Line::from(vec![
            Span::styled(" [Phase 2: Trimming]        ", Style::default().fg(Color::Yellow)),
            Span::raw("Isolating failing boundary edges with 500ms watchdog timeouts."),
        ]),
        Line::from(vec![
            Span::styled(" [Phase 3: Scraping]        ", Style::default().fg(Color::Magenta)),
            Span::raw("Single 512-byte sector salvage to maximize byte-level recovery."),
        ]),
    ];
    let stages_para = Paragraph::new(stage_info)
        .block(Block::default().borders(Borders::ALL).title(" Algorithmic Rescue Phases "));
    f.render_widget(stages_para, stage_chunks[1]);

    // 4. Action bar
    let action_text = if is_editing_path {
        Line::from(vec![
            Span::styled(" [Type Path] ", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
            Span::styled(" [Enter: Save] ", Style::default().fg(Color::Green)),
            Span::styled(" [Esc: Cancel] ", Style::default().fg(Color::Red)),
        ])
    } else {
        Line::from(vec![
            Span::styled(
                if imaging_in_progress { " [Space: Pause/Resume] [X: Stop Session] " } else { " [I: Start Imaging] " },
                Style::default().fg(Color::Green).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" [M: Switch Mode] ", Style::default().fg(Color::Cyan)),
            Span::styled(" [P: Toggle Sparse] ", Style::default().fg(Color::Magenta)),
            Span::styled(" [O / L: Destination] ", Style::default().fg(Color::Yellow)),
            Span::styled(" [Esc: Dashboard] ", Style::default().fg(Color::White)),
            if !status_msg.is_empty() {
                Span::styled(format!(" [{}] ", status_msg), Style::default().fg(if status_msg.starts_with("Error") { Color::Red } else { Color::Green }).add_modifier(Modifier::BOLD))
            } else if imaging_in_progress {
                Span::styled(" [STATUS: ACTIVE RUNNING] ", Style::default().fg(Color::Green))
            } else {
                Span::styled(" [STATUS: STANDBY] ", Style::default().fg(Color::DarkGray))
            },
        ])
    };
    let action_para = Paragraph::new(action_text).block(Block::default().borders(Borders::ALL));
    f.render_widget(action_para, chunks[3]);
}
