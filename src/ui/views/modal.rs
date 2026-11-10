use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
    Frame,
};

use crate::common::types::DiskInfo;
use crate::core::surface::probe::FakeProbeOutcome;

pub fn render_safety_modal(
    f: &mut Frame,
    area: Rect,
    disk: &DiskInfo,
    input_serial: &str,
    is_error: bool,
) {
    // Center popup: 64 cols x 12 rows
    let popup_width = 66.min(area.width.saturating_sub(4));
    let popup_height = 12.min(area.height.saturating_sub(4));

    let vertical_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length((area.height.saturating_sub(popup_height)) / 2),
            Constraint::Length(popup_height),
            Constraint::Min(0),
        ])
        .split(area);

    let horizontal_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length((area.width.saturating_sub(popup_width)) / 2),
            Constraint::Length(popup_width),
            Constraint::Min(0),
        ])
        .split(vertical_chunks[1]);

    let modal_area = horizontal_chunks[1];

    // Clear background
    f.render_widget(Clear, modal_area);

    let modal_block = Block::default()
        .borders(Borders::ALL)
        .title(" ⚠️  CRITICAL DESTRUCTIVE SAFETY GATE ⚠️  ")
        .title_alignment(Alignment::Center)
        .border_style(Style::default().fg(Color::Red).add_modifier(Modifier::BOLD));

    let content_chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints([
            Constraint::Length(2), // Warning text
            Constraint::Length(2), // Target drive & serial info
            Constraint::Length(3), // Input box
            Constraint::Length(1), // Footer keys
        ])
        .split(modal_area);

    let warning_para = Paragraph::new(
        "This operation writes test patterns directly to raw sectors.\nAll filesystems and data on this drive WILL BE DESTROYED.",
    )
    .style(Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD));

    let target_para = Paragraph::new(format!(
        "Target: PhysicalDrive{} ({})\nTo proceed, type exact serial: {}",
        disk.disk_number, disk.model, disk.serial_number
    ));

    let input_style = if is_error {
        Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)
    };

    let input_para = Paragraph::new(format!("> {}█", input_serial))
        .style(input_style)
        .block(Block::default().borders(Borders::ALL).title(" Type Serial to Unlock "));

    let footer_para = Paragraph::new("[Enter: Execute Destructive Probe]    [Esc: Cancel Safe]")
        .style(Style::default().fg(Color::DarkGray));

    f.render_widget(modal_block, modal_area);
    f.render_widget(warning_para, content_chunks[0]);
    f.render_widget(target_para, content_chunks[1]);
    f.render_widget(input_para, content_chunks[2]);
    f.render_widget(footer_para, content_chunks[3]);
}

pub fn render_probe_outcome_modal(
    f: &mut Frame,
    area: Rect,
    disk: &DiskInfo,
    outcome: &Result<FakeProbeOutcome, String>,
) {
    let popup_width = 76.min(area.width.saturating_sub(4));
    let popup_height = 18.min(area.height.saturating_sub(4));

    let vertical_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length((area.height.saturating_sub(popup_height)) / 2),
            Constraint::Length(popup_height),
            Constraint::Min(0),
        ])
        .split(area);

    let horizontal_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length((area.width.saturating_sub(popup_width)) / 2),
            Constraint::Length(popup_width),
            Constraint::Min(0),
        ])
        .split(vertical_chunks[1]);

    let modal_area = horizontal_chunks[1];
    f.render_widget(Clear, modal_area);

    let (title, border_color) = match outcome {
        Ok(out) if out.is_fake => (" 🚨 FAKE PROBER: COUNTERFEIT STORAGE DETECTED! 🚨 ", Color::Red),
        Ok(_) => (" ✅ FAKE PROBER: GENUINE DRIVE VERIFIED ✅ ", Color::Green),
        Err(_) => (" ❌ FAKE PROBER: EXECUTION ERROR ❌ ", Color::Yellow),
    };

    let modal_block = Block::default()
        .borders(Borders::ALL)
        .title(title)
        .title_alignment(Alignment::Center)
        .border_style(Style::default().fg(border_color).add_modifier(Modifier::BOLD));

    let content_chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints([
            Constraint::Length(3), // Summary info
            Constraint::Length(9), // LBA Sparse Probe Boundary Map / Graph
            Constraint::Length(1), // Dismiss footer
        ])
        .split(modal_area);

    match outcome {
        Ok(out) => {
            let declared_gb = out.declared_capacity_bytes as f64 / 1e9;
            let verified_gb = out.verified_capacity_bytes as f64 / 1e9;
            let status_line = if out.is_fake {
                format!(
                    "Drive PhysicalDrive{} is COUNTERFEIT! Firmware: {:.2} GB vs Real Flash: {:.2} GB\nAnomaly: {}",
                    disk.disk_number, declared_gb, verified_gb, out.anomaly.as_deref().unwrap_or("Hardware address overwrite")
                )
            } else {
                format!(
                    "Drive PhysicalDrive{} is 100% GENUINE!\nDeclared: {:.2} GB | Verified Physical Flash: {:.2} GB (Zero wrap-around detected)",
                    disk.disk_number, declared_gb, verified_gb
                )
            };

            let summary_style = if out.is_fake {
                Style::default().fg(Color::LightRed).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::LightGreen).add_modifier(Modifier::BOLD)
            };

            let summary_para = Paragraph::new(status_line).style(summary_style);
            f.render_widget(summary_para, content_chunks[0]);

            // The Sparse Boundary Graph / Map
            let mut graph_lines = vec![
                Line::from(vec![
                    Span::styled(" [Sparse LBA Hardware Probe Graph] ", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
                ]),
                Line::from(vec![
                    Span::styled("  [LBA 64 (32 KB)] ", Style::default().fg(Color::White)),
                    Span::styled("───► ", Style::default().fg(Color::DarkGray)),
                    Span::styled("[ Anchor Established ]", Style::default().fg(Color::Green)),
                ]),
            ];

            let mut step = 1024 * 1024 * 1024u64;
            while step < out.declared_capacity_bytes {
                let gb = step as f64 / 1e9;
                let (indicator, color) = if out.is_fake && step > out.verified_capacity_bytes {
                    ("[ ✗ FAKE / PHANTOM CELL ]", Color::Red)
                } else {
                    ("[ ✓ Genuine Flash Cell ]", Color::Green)
                };
                graph_lines.push(Line::from(vec![
                    Span::styled(format!("  [LBA {:.1} GB]       ", gb), Style::default().fg(Color::White)),
                    Span::styled("───► ", Style::default().fg(Color::DarkGray)),
                    Span::styled(indicator, Style::default().fg(color)),
                ]));
                step *= 2;
                if graph_lines.len() >= 6 {
                    break;
                }
            }

            let end_gb = out.declared_capacity_bytes as f64 / 1e9;
            let (end_ind, end_color) = if out.is_fake {
                ("[ ✗ End Boundary OVERWRITE ]", Color::Red)
            } else {
                ("[ ✓ End Boundary VERIFIED ]", Color::Green)
            };
            graph_lines.push(Line::from(vec![
                Span::styled(format!("  [End {:.1} GB]       ", end_gb), Style::default().fg(Color::White)),
                Span::styled("───► ", Style::default().fg(Color::DarkGray)),
                Span::styled(end_ind, Style::default().fg(end_color).add_modifier(Modifier::BOLD)),
            ]));

            let graph_para = Paragraph::new(graph_lines).block(Block::default().borders(Borders::ALL).title(" LBA Boundary Verification Graph "));
            f.render_widget(graph_para, content_chunks[1]);
        }
        Err(err) => {
            let err_para = Paragraph::new(format!(
                "Probe encountered an unrecoverable Win32 I/O error:\n\n{}\n\nCommon Causes: Drive firmware locked read-only, hardware write failure, or partition lock denied.",
                err
            ))
            .style(Style::default().fg(Color::Red));
            f.render_widget(err_para, content_chunks[0]);
        }
    }

    let dismiss_para = Paragraph::new("Press [Enter] or [Esc] to dismiss report and return to dashboard.")
        .style(Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC))
        .alignment(Alignment::Center);
    f.render_widget(dismiss_para, content_chunks[2]);

    f.render_widget(modal_block, modal_area);
}
