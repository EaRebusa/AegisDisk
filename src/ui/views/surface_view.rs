use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};

use crate::common::types::DiskInfo;
use crate::core::surface::latency_map::{LatencyBucket, SurfaceScanStats};

pub fn render_surface_view(
    f: &mut Frame,
    area: Rect,
    disk: &DiskInfo,
    blocks: &[LatencyBucket],
    stats: &SurfaceScanStats,
    is_paused: bool,
    elapsed_secs: u64,
) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // Top info header
            Constraint::Min(10),   // Block grid matrix
            Constraint::Length(4), // Legend & metrics
            Constraint::Length(3), // Action bar
        ])
        .split(area);

    // 1. Top Header
    let gb_total = disk.geometry.total_bytes as f64 / 1e9;
    let progress_pct = if disk.geometry.total_bytes > 0 {
        ((stats.total_bytes_scanned as f64 / disk.geometry.total_bytes as f64) * 100.0).clamp(0.0, 100.0)
    } else {
        0.0
    };

    let header_text = format!(
        " Target: PhysicalDrive{} | Range: 0 -> {:.0} GB | Block Size: 256 KB | Progress: {:.1}% {}",
        disk.disk_number,
        gb_total,
        progress_pct,
        if is_paused { "[PAUSED]" } else { "[SCANNING]" }
    );
    let header_para = Paragraph::new(header_text).block(
        Block::default()
            .borders(Borders::ALL)
            .title(format!(" AegisDisk - Surface Health & Block Grid [PhysicalDrive{}] ", disk.disk_number)),
    );
    f.render_widget(header_para, chunks[0]);

    // 2. Block Grid Matrix
    // Render blocks across the inner area with auto-scrolling
    let inner_width = chunks[1].width.saturating_sub(2) as usize;
    let inner_height = chunks[1].height.saturating_sub(2) as usize;
    let max_display_blocks = inner_width * inner_height;

    let display_slice = if blocks.len() > max_display_blocks {
        &blocks[blocks.len() - max_display_blocks..]
    } else {
        blocks
    };

    let mut lines = Vec::new();
    let mut current_line = Vec::new();

    for (idx, bucket) in display_slice.iter().enumerate() {
        let style = match bucket {
            LatencyBucket::Fast => Style::default().fg(Color::Green),
            LatencyBucket::Normal => Style::default().fg(Color::LightGreen),
            LatencyBucket::Sluggish => Style::default().fg(Color::Yellow),
            LatencyBucket::Critical => Style::default().fg(Color::Rgb(255, 140, 0)),
            LatencyBucket::Timeout => Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            LatencyBucket::ReadError(_) => Style::default().fg(Color::Magenta).add_modifier(Modifier::BOLD),
        };

        current_line.push(Span::styled(bucket.symbol(), style));

        if (idx + 1) % inner_width == 0 {
            lines.push(Line::from(std::mem::take(&mut current_line)));
        }
    }
    if !current_line.is_empty() {
        lines.push(Line::from(current_line));
    }

    let grid_para = Paragraph::new(lines).block(Block::default().borders(Borders::ALL).title(" Latency Grid "));
    f.render_widget(grid_para, chunks[1]);

    // 3. Legend & Metrics
    let bottom_split = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
        .split(chunks[2]);

    let legend_lines = vec![
        Line::from(vec![
            Span::styled("■ ", Style::default().fg(Color::Green)),
            Span::raw(format!("<10ms ({})   ", stats.fast_count)),
            Span::styled("■ ", Style::default().fg(Color::Yellow)),
            Span::raw(format!("<150ms ({})   ", stats.sluggish_count)),
            Span::styled("■ ", Style::default().fg(Color::Red)),
            Span::raw(format!(">500ms ({})", stats.timeout_count)),
        ]),
        Line::from(vec![
            Span::styled("■ ", Style::default().fg(Color::LightGreen)),
            Span::raw(format!("<50ms ({})   ", stats.normal_count)),
            Span::styled("■ ", Style::default().fg(Color::Rgb(255, 140, 0))),
            Span::raw(format!("<500ms ({})   ", stats.critical_count)),
            Span::styled("X ", Style::default().fg(Color::Magenta)),
            Span::raw(format!("Bad ({})", stats.error_count)),
        ]),
    ];
    let legend_para = Paragraph::new(legend_lines).block(Block::default().borders(Borders::ALL).title(" Legend "));
    f.render_widget(legend_para, bottom_split[0]);

    let mins = elapsed_secs / 60;
    let secs = elapsed_secs % 60;
    let metrics_lines = vec![
        Line::from(format!(" Elapsed: {:02}:{:02} | Total Scanned: {:.1} MB", mins, secs, stats.total_bytes_scanned as f64 / 1e6)),
        Line::from(format!(" Bad Blocks: {} | Total Blocks: {}", stats.error_count, stats.total_blocks)),
    ];
    let metrics_para = Paragraph::new(metrics_lines).block(Block::default().borders(Borders::ALL).title(" Scan Metrics "));
    f.render_widget(metrics_para, bottom_split[1]);

    // 4. Action bar
    let action_text = Line::from(vec![
        Span::styled(" [Space: Pause/Resume] ", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
        Span::styled(" [Esc: Return to Dashboard] ", Style::default().fg(Color::White)),
    ]);
    let action_para = Paragraph::new(action_text).block(Block::default().borders(Borders::ALL));
    f.render_widget(action_para, chunks[3]);
}
