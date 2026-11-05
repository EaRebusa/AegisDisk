use std::path::Path;
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Gauge, Paragraph, Row, Table},
    Frame,
};

use crate::core::recovery::carver::CarvedFile;

pub fn render_carver_view(
    f: &mut Frame,
    area: Rect,
    files: &[CarvedFile],
    selected_index: usize,
    sample_hex: &[u8],
    dest_dir: &Path,
    is_editing_dest: bool,
    is_editing_source: bool,
    path_input: &str,
    status_msg: &str,
    carver_running: bool,
    scanned_bytes: u64,
    total_bytes: u64,
    speed_mb_s: f64,
    source_file: Option<&Path>,
    disk_number: u32,
    plaintext_gate: crate::core::recovery::carver::PlaintextGate,
) {
    let show_hex = area.height >= 26;
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // Header
            Constraint::Length(3), // Source & Destination Banner
            Constraint::Length(3), // Live Ticker Gauge
            Constraint::Min(4),    // Recovered files list table (flexibly expands on tall screens)
            Constraint::Length(if show_hex { 7 } else { 0 }), // Hex Inspector pane
            Constraint::Length(3), // Action bar
        ])
        .split(area);

    // 1. Header with Source info
    let source_str = match source_file {
        Some(p) => format!("Image File: {}", p.display()),
        None => format!("PhysicalDrive{} (Direct I/O Live Media)", disk_number),
    };
    let header_para = Paragraph::new(format!(" Source Target: {} | Engine: Two-Tier Cascade (Metadata Undelete + Direct I/O Carver)", source_str))
        .block(Block::default().borders(Borders::ALL).title(" AegisDisk - Resilient Platter Carver & Parser "));
    f.render_widget(header_para, chunks[0]);

    // 2. Salvage Source / Destination Banner
    let banner_text = if is_editing_source {
        format!(" > ENTER IMAGE FILE PATH (.raw/.img) OR 'live' FOR DRIVE: {} [ENTER: Confirm | ESC: Cancel]", path_input)
    } else if is_editing_dest {
        format!(" > ENTER NEW SALVAGE DIRECTORY: {} [ENTER: Save | ESC: Cancel]", path_input)
    } else {
        format!(" Salvage Output Path: {}  [Press 'O'/'L' to change dir | 'T' to target .raw file]", dest_dir.display())
    };
    let banner_style = if is_editing_source || is_editing_dest {
        Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::Cyan)
    };
    let banner_title = if is_editing_source {
        " EDITING CARVER SOURCE TARGET "
    } else if is_editing_dest {
        " EDITING SALVAGE DESTINATION "
    } else {
        " Carver Paths & Directives "
    };
    let banner_para = Paragraph::new(banner_text)
        .style(banner_style)
        .block(Block::default().borders(Borders::ALL).title(banner_title));
    f.render_widget(banner_para, chunks[1]);

    // 3. Live Ticker Progress Gauge
    let ratio = if total_bytes > 0 {
        (scanned_bytes as f64 / total_bytes as f64).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let gauge_title = if carver_running {
        " Carver Direct I/O Platter Progress [Active Scan] "
    } else {
        " Carver Direct I/O Platter Progress [Idle] "
    };
    let gauge_label = if total_bytes > 0 {
        format!(
            "{:.2} GB / {:.2} GB ({:.1}%) | Speed: {:.1} MB/s | Salvaged: {} files",
            scanned_bytes as f64 / 1e9,
            total_bytes as f64 / 1e9,
            ratio * 100.0,
            speed_mb_s,
            files.len()
        )
    } else {
        format!("Ready to scan (Press 'S' to begin) | Salvaged: {} files", files.len())
    };
    let gauge = Gauge::default()
        .block(Block::default().borders(Borders::ALL).title(gauge_title))
        .gauge_style(
            Style::default()
                .fg(if carver_running { Color::Cyan } else { Color::DarkGray })
                .bg(Color::Black),
        )
        .ratio(ratio)
        .label(gauge_label);
    f.render_widget(gauge, chunks[2]);

    // 4. Recovered Files Table (with automatic scrolling window around selected_index)
    let visible_rows = chunks[3].height.saturating_sub(4) as usize; // account for borders & header
    let total_items = files.len();
    let scroll_offset = if total_items > visible_rows && visible_rows > 0 {
        if selected_index >= visible_rows {
            (selected_index + 1).saturating_sub(visible_rows).min(total_items.saturating_sub(visible_rows))
        } else {
            0
        }
    } else {
        0
    };

    let slice_files = if files.is_empty() {
        &files[..]
    } else {
        let end = (scroll_offset + visible_rows).min(total_items);
        &files[scroll_offset..end]
    };

    let rows: Vec<Row> = if files.is_empty() {
        vec![Row::new(vec![
            Span::raw("NO_FILES"),
            Span::raw(if carver_running { "Scanning sectors in real-time..." } else { "No salvaged files found yet in target space" }),
            Span::raw("--"),
            Span::raw("--"),
            Span::raw("--"),
        ])]
    } else {
        slice_files
            .iter()
            .enumerate()
            .map(|(rel_idx, cf)| {
                let actual_idx = scroll_offset + rel_idx;
                let is_sel = actual_idx == selected_index;
                let row_style = if is_sel {
                    Style::default().bg(Color::DarkGray).fg(Color::White).add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                };

                let filename = cf.output_path.file_name().unwrap_or_default().to_string_lossy();
                let is_metadata = cf.output_path.to_string_lossy().contains("intact_metadata");
                let status_badge = if is_metadata {
                    Span::styled("INTACT", Style::default().fg(Color::LightGreen).add_modifier(Modifier::BOLD))
                } else {
                    Span::styled("CARVED", Style::default().fg(Color::Green))
                };

                Row::new(vec![
                    status_badge,
                    Span::raw(filename.to_string()),
                    Span::raw(format!("{:?}", cf.file_type)),
                    Span::raw(format!("0x{:08X}", cf.offset)),
                    Span::raw(format!("{:.1} KB", cf.size_bytes as f64 / 1024.0)),
                ])
                .style(row_style)
            })
            .collect()
    };

    let table_title = if total_items > 0 {
        format!(" Recoverable Files Found [Showing {}-{} of {}] ", scroll_offset + 1, (scroll_offset + slice_files.len()).min(total_items), total_items)
    } else {
        " Recoverable Files Found ".to_string()
    };

    let table = Table::new(
        rows,
        [
            Constraint::Length(12),
            Constraint::Percentage(40),
            Constraint::Percentage(15),
            Constraint::Percentage(20),
            Constraint::Percentage(15),
        ],
    )
    .header(
        Row::new(vec!["STATUS", "FILENAME / ID", "FORMAT", "OFFSET", "EST. SIZE"])
            .style(Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
    )
    .block(Block::default().borders(Borders::ALL).title(table_title));
    f.render_widget(table, chunks[3]);

    // 5. Hex Inspector Pane
    let mut hex_lines = Vec::new();
    for chunk in sample_hex.chunks(16) {
        let hex_str: Vec<String> = chunk.iter().map(|b| format!("{:02X}", b)).collect();
        let ascii_str: String = chunk
            .iter()
            .map(|&b| if b >= 0x20 && b <= 0x7E { b as char } else { '.' })
            .collect();

        hex_lines.push(Line::from(vec![
            Span::raw(format!("{:<48}", hex_str.join(" "))),
            Span::styled(format!(" |{}|", ascii_str), Style::default().fg(Color::Cyan)),
        ]));
    }

    let hex_para = Paragraph::new(hex_lines)
        .block(Block::default().borders(Borders::ALL).title(" Hex Inspector (Selected Header) "));
    if show_hex && chunks[4].height > 0 {
        f.render_widget(hex_para, chunks[4]);
    }

    // 6. Action bar
    let action_text = if is_editing_dest || is_editing_source {
        Line::from(vec![
            Span::styled(" [Type Path] ", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
            Span::styled(" [Enter: Confirm] ", Style::default().fg(Color::Green)),
            Span::styled(" [Esc: Cancel] ", Style::default().fg(Color::Red)),
        ])
    } else {
        Line::from(vec![
            Span::styled(
                if carver_running { " [Space: Pause/Resume] [X: Stop Scan] " } else { " [S: Start Carve Scan] " },
                Style::default().fg(Color::Green).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!(" [U: Plaintext: {}] ", plaintext_gate.label()),
                Style::default().fg(if plaintext_gate.min_bytes().is_some() { Color::Green } else { Color::DarkGray }),
            ),
            Span::styled(" [T: Target Image File] ", Style::default().fg(Color::Magenta)),
            Span::styled(" [O / L: Destination] ", Style::default().fg(Color::Yellow)),
            Span::styled(" [Esc: Dashboard] ", Style::default().fg(Color::White)),
            if !status_msg.is_empty() {
                Span::styled(format!(" [{}] ", status_msg), Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))
            } else {
                Span::raw("")
            },
        ])
    };

    let action_para = Paragraph::new(action_text)
        .block(Block::default().borders(Borders::ALL).title(" Control Actions "));
    f.render_widget(action_para, chunks[5]);
}
