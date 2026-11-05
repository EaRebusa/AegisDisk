use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Gauge, Paragraph, Row, Table},
    Frame,
};

use crate::common::types::DiskInfo;
use crate::core::telemetry::DriveTelemetry;

pub fn render_dashboard(
    f: &mut Frame,
    area: Rect,
    disks: &[DiskInfo],
    selected_index: usize,
    telemetry: Option<&DriveTelemetry>,
    is_elevated: bool,
) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(8), // Drive list (compact)
            Constraint::Length(6), // In-depth hardware telemetry pane
            Constraint::Min(10),   // SMART / NVMe table & Lifetime gauges
            Constraint::Length(3), // Function keys bar
        ])
        .split(area);

    // 1. Drive List Pane
    let selected_disk = disks.get(selected_index);
    let rows: Vec<Row> = disks
        .iter()
        .enumerate()
        .map(|(i, d)| {
            let prefix = if i == selected_index { ">" } else { " " };
            let gb = d.geometry.total_bytes as f64 / 1e9;
            let bus = format!("{:?}", d.bus_type);
            let status_badge = if d.is_system_boot {
                Span::styled("[SYSTEM - PROTECTED]", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))
            } else {
                Span::styled("[ACCESSIBLE]", Style::default().fg(Color::Green))
            };

            let row_style = if i == selected_index {
                Style::default().bg(Color::DarkGray).fg(Color::White).add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };

            let cap_str = match d.used_bytes {
                Some(u) => {
                    let total_gb = d.geometry.total_bytes as f64 / 1e9;
                    let free_gb = (d.geometry.total_bytes.saturating_sub(u)) as f64 / 1e9;
                    let pct = if d.geometry.total_bytes > 0 {
                        (u as f64 / d.geometry.total_bytes as f64) * 100.0
                    } else {
                        0.0
                    };
                    format!("{:.0}GB free/{:.0}GB ({:.0}%)", free_gb, total_gb, pct)
                }
                None => format!("{:.1} GB", gb),
            };

            Row::new(vec![
                Span::raw(format!("{} [{}]", prefix, d.disk_number)),
                Span::raw(d.model.clone()),
                Span::raw(cap_str),
                Span::raw(bus),
                status_badge,
            ])
            .style(row_style)
        })
        .collect();

    let drive_table = Table::new(
        rows,
        [
            Constraint::Length(8),
            Constraint::Percentage(30),
            Constraint::Percentage(25),
            Constraint::Percentage(15),
            Constraint::Percentage(30),
        ],
    )
    .header(
        Row::new(vec!["SEL", "MODEL", "CAPACITY (USED)", "INTERFACE", "STATUS"])
            .style(Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
    )
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title(format!(
                " AegisDisk v0.1.0 ──── [{}] ── [Q: Quit] ",
                if is_elevated { "ADMIN ELEVATED" } else { "UNPRIVILEGED" }
            )),
    );

    f.render_widget(drive_table, chunks[0]);

    // 2. Device Telemetry Summary
    let (model, serial, bus_str) = match selected_disk {
        Some(d) => (d.model.as_str(), d.serial_number.as_str(), format!("{:?}", d.bus_type)),
        None => ("None", "None", "None".to_string()),
    };

    let proto_str = telemetry
        .and_then(|t| t.protocol.as_ref())
        .map(|p| format!("{:?}", p))
        .unwrap_or_else(|| "Direct IOCTL".to_string());

    let poh_str = telemetry
        .and_then(|t| t.power_on_hours)
        .map(|poh| format!("{} hrs ({:.1} days)", poh, poh as f64 / 24.0))
        .unwrap_or_else(|| "N/A".to_string());

    let cycles_str = telemetry
        .and_then(|t| t.power_cycles)
        .map(|c| c.to_string())
        .unwrap_or_else(|| "N/A".to_string());

    let rw_str = match (telemetry.and_then(|t| t.data_read_tb), telemetry.and_then(|t| t.data_written_tb)) {
        (Some(r), Some(w)) => format!("Read: {:.2} TB | Written: {:.2} TB", r, w),
        _ => "Read/Write Stats: N/A".to_string(),
    };

    let spare_str = telemetry
        .and_then(|t| t.available_spare_percentage)
        .map(|s| format!("{}%", s))
        .unwrap_or_else(|| "N/A".to_string());

    let wear_str = telemetry
        .and_then(|t| t.wear_percentage)
        .map(|w| format!("{}% used", w))
        .unwrap_or_else(|| "N/A".to_string());

    let summary_text = format!(
        " Model: {} | Bus: {} | Protocol: {}\n Serial: {}\n Power-On: {} | Cycles: {} | {}\n Spare Reserve: {} | Flash Wear: {}",
        model, bus_str, proto_str, serial, poh_str, cycles_str, rw_str, spare_str, wear_str
    );

    let summary_para = Paragraph::new(summary_text)
        .block(Block::default().borders(Borders::ALL).title(" Hardware & Telemetry Details "));
    f.render_widget(summary_para, chunks[1]);

    // 3. Middle split: Left = Telemetry/SMART indicators, Right = Lifetime gauges
    let middle_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
        .split(chunks[2]);

    let is_nvme = telemetry
        .and_then(|t| t.protocol.as_ref())
        .map(|p| matches!(p, crate::core::telemetry::types::TelemetryProtocol::NativeNvme))
        .unwrap_or(false);

    let is_generic = telemetry
        .and_then(|t| t.protocol.as_ref())
        .map(|p| matches!(p, crate::core::telemetry::types::TelemetryProtocol::GenericBlock))
        .unwrap_or(false);

    // Left: S.M.A.R.T. / NVMe Health Table
    let (table_title, header_col1, smart_rows) = if is_nvme {
        let t = telemetry.unwrap();
        let rows = vec![
            Row::new(vec![
                Span::raw("SP"),
                Span::raw("Available Spare Reserve"),
                Span::raw(format!("{}%", t.available_spare_percentage.unwrap_or(100))),
                if t.available_spare_percentage.unwrap_or(100) < 10 {
                    Span::styled("[CRIT]", Style::default().fg(Color::Red).add_modifier(Modifier::BOLD))
                } else {
                    Span::styled("[ OK ]", Style::default().fg(Color::Green))
                },
            ]),
            Row::new(vec![
                Span::raw("WR"),
                Span::raw("Percentage Used (Flash Wear)"),
                Span::raw(format!("{}%", t.wear_percentage.unwrap_or(0))),
                if t.wear_percentage.unwrap_or(0) >= 90 {
                    Span::styled("[WARN]", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD))
                } else {
                    Span::styled("[ OK ]", Style::default().fg(Color::Green))
                },
            ]),
            Row::new(vec![
                Span::raw("UC"),
                Span::raw("Media & Data Integrity Errors"),
                Span::raw(t.uncorrectable_sectors.unwrap_or(0).to_string()),
                if t.uncorrectable_sectors.unwrap_or(0) > 0 {
                    Span::styled("[CRIT]", Style::default().fg(Color::Red).add_modifier(Modifier::BOLD))
                } else {
                    Span::styled("[ OK ]", Style::default().fg(Color::Green))
                },
            ]),
            Row::new(vec![
                Span::raw("CW"),
                Span::raw("Critical Warning Flags"),
                Span::raw(format!("0x{:02X}", t.critical_warning.unwrap_or(0))),
                if t.critical_warning.unwrap_or(0) > 0 {
                    Span::styled("[WARN]", Style::default().fg(Color::Red).add_modifier(Modifier::BOLD))
                } else {
                    Span::styled("[ OK ]", Style::default().fg(Color::Green))
                },
            ]),
        ];
        (" NVMe Health & Flash Telemetry ", "TAG", rows)
    } else if is_generic {
        let rows = vec![
            Row::new(vec![
                Span::raw("BK"),
                Span::raw("Block Device Direct I/O"),
                Span::raw("Active"),
                Span::styled("[ OK ]", Style::default().fg(Color::Green)),
            ]),
            Row::new(vec![
                Span::raw("MD"),
                Span::raw("Storage Controller Type"),
                Span::raw("Generic/SD/Flash"),
                Span::styled("[ OK ]", Style::default().fg(Color::Cyan)),
            ]),
            Row::new(vec![
                Span::raw("SM"),
                Span::raw("Internal S.M.A.R.T. Registers"),
                Span::raw("Controller Bypass"),
                Span::styled("[INFO]", Style::default().fg(Color::Yellow)),
            ]),
            Row::new(vec![
                Span::raw("AC"),
                Span::raw("Surface Scanning / Imaging"),
                Span::raw("Supported"),
                Span::styled("[ OK ]", Style::default().fg(Color::Green)),
            ]),
        ];
        (" Storage Controller & Media Status ", "TAG", rows)
    } else if let Some(t) = telemetry {
        let rows = vec![
            Row::new(vec![
                Span::raw("05"),
                Span::raw("Reallocated Sectors"),
                Span::raw(t.reallocated_sectors.map(|v| v.to_string()).unwrap_or_else(|| "0".to_string())),
                if t.reallocated_sectors.unwrap_or(0) > 0 {
                    Span::styled("[WARN]", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD))
                } else {
                    Span::styled("[ OK ]", Style::default().fg(Color::Green))
                },
            ]),
            Row::new(vec![
                Span::raw("BB"),
                Span::raw("Reported Uncorrectable"),
                Span::raw(t.uncorrectable_sectors.map(|v| v.to_string()).unwrap_or_else(|| "0".to_string())),
                if t.uncorrectable_sectors.unwrap_or(0) > 0 {
                    Span::styled("[WARN]", Style::default().fg(Color::Red).add_modifier(Modifier::BOLD))
                } else {
                    Span::styled("[ OK ]", Style::default().fg(Color::Green))
                },
            ]),
            Row::new(vec![
                Span::raw("C5"),
                Span::raw("Current Pending Sectors"),
                Span::raw(t.pending_sectors.map(|v| v.to_string()).unwrap_or_else(|| "0".to_string())),
                if t.pending_sectors.unwrap_or(0) > 0 {
                    Span::styled("[CRIT]", Style::default().fg(Color::Red).add_modifier(Modifier::BOLD))
                } else {
                    Span::styled("[ OK ]", Style::default().fg(Color::Green))
                },
            ]),
            Row::new(vec![
                Span::raw("C6"),
                Span::raw("Offline Uncorrectable"),
                Span::raw("0"),
                Span::styled("[ OK ]", Style::default().fg(Color::Green)),
            ]),
        ];
        (" S.M.A.R.T. Key Indicators ", "ID", rows)
    } else {
        (
            " S.M.A.R.T. Key Indicators ",
            "ID",
            vec![Row::new(vec![
                Span::raw("--"),
                Span::raw("No SMART / NVMe Telemetry Available"),
                Span::raw("--"),
                Span::raw("--"),
            ])],
        )
    };

    let smart_table = Table::new(
        smart_rows,
        [
            Constraint::Length(6),
            Constraint::Percentage(55),
            Constraint::Percentage(20),
            Constraint::Percentage(25),
        ],
    )
    .header(
        Row::new(vec![header_col1, "INDICATOR / ATTRIBUTE", "RAW VAL", "STATUS"])
            .style(Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
    )
    .block(Block::default().borders(Borders::ALL).title(table_title));

    f.render_widget(smart_table, middle_chunks[0]);

    // Right: Lifetime Health Gauges
    let right_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // Life / Spare Gauge
            Constraint::Length(3), // Temp Gauge
            Constraint::Min(2),    // Status advice
        ])
        .split(middle_chunks[1]);

    let wear_pct = telemetry.and_then(|t| t.wear_percentage).unwrap_or(0);
    let life_pct = 100u8.saturating_sub(wear_pct);

    let life_gauge = Gauge::default()
        .block(Block::default().title(" Spare / Drive Lifespan "))
        .gauge_style(if life_pct > 50 { Style::default().fg(Color::Green) } else { Style::default().fg(Color::Red) })
        .percent(life_pct as u16);
    f.render_widget(life_gauge, right_chunks[0]);

    let temp_val = telemetry.and_then(|t| t.temperature_c).unwrap_or(35);
    let mut max_safe = telemetry.and_then(|t| t.max_safe_temp_c).unwrap_or(if is_nvme { 70 } else { 50 });
    let mut crit_temp = telemetry.and_then(|t| t.critical_temp_c).unwrap_or(if is_nvme { 80 } else { 65 });

    // Sanitize any corrupt or inverted hardware registers
    if max_safe >= crit_temp || max_safe < 40 || crit_temp < 50 {
        max_safe = if is_nvme { 70 } else { 50 };
        crit_temp = if is_nvme { 80 } else { 65 };
    }

    let temp_pct = ((temp_val as f64 / crit_temp as f64) * 100.0).clamp(0.0, 100.0) as u16;
    let gauge_color = if temp_val < max_safe {
        Color::Cyan
    } else if temp_val < crit_temp {
        Color::Yellow
    } else {
        Color::Red
    };

    let temp_gauge = Gauge::default()
        .block(Block::default().title(format!(" Temperature: {}°C (Safe: {}°C | Crit: {}°C) ", temp_val, max_safe, crit_temp)))
        .gauge_style(Style::default().fg(gauge_color))
        .percent(temp_pct);
    f.render_widget(temp_gauge, right_chunks[1]);

    let advice_style = if t_has_issues(telemetry) {
        Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::Green)
    };

    let advice_text = if t_has_issues(telemetry) {
        " Status: Caution! Drive has pending/reallocated sectors. Multi-pass raw clone advised."
    } else {
        " Status: Drive healthy. Normal operation."
    };

    let advice_para = Paragraph::new(advice_text)
        .style(advice_style)
        .block(Block::default().borders(Borders::ALL).title(" Health Evaluation "));
    f.render_widget(advice_para, right_chunks[2]);

    // 4. Function keys bar
    let nav_text = Line::from(vec![
        Span::styled(" [F1: Dashboard] ", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
        Span::styled(" [F2: Surface Map] ", Style::default().fg(Color::White)),
        Span::styled(" [F3: Fake Prober] ", Style::default().fg(Color::Yellow)),
        Span::styled(" [F4: Imager] ", Style::default().fg(Color::LightBlue)),
        Span::styled(" [F5: Carver] ", Style::default().fg(Color::Magenta)),
        Span::styled(" [Tab: Next Disk] ", Style::default().fg(Color::DarkGray)),
    ]);
    let nav_para = Paragraph::new(nav_text).block(Block::default().borders(Borders::ALL));
    f.render_widget(nav_para, chunks[3]);
}

fn t_has_issues(telemetry: Option<&DriveTelemetry>) -> bool {
    if let Some(t) = telemetry {
        t.reallocated_sectors.unwrap_or(0) > 0 || t.pending_sectors.unwrap_or(0) > 0 || t.uncorrectable_sectors.unwrap_or(0) > 0
    } else {
        false
    }
}
