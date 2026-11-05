use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, Clear, Paragraph},
    Frame,
};

use crate::common::types::DiskInfo;

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
