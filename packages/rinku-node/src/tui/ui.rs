use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph, Row, Table, Tabs},
    Frame,
};

use super::app::{App, Tab};

pub fn draw(frame: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(0),
            Constraint::Length(3),
        ])
        .split(frame.size());

    draw_header(frame, app, chunks[0]);
    draw_tabs(frame, app, chunks[0]);

    match app.current_tab {
        Tab::Dashboard => draw_dashboard(frame, app, chunks[1]),
        Tab::Network => draw_network(frame, app, chunks[1]),
        Tab::Validator => draw_validator(frame, app, chunks[1]),
        Tab::DAG => draw_dag(frame, app, chunks[1]),
        Tab::Logs => draw_logs(frame, app, chunks[1]),
    }

    draw_footer(frame, app, chunks[2]);
}

fn draw_header(frame: &mut Frame, app: &App, area: Rect) {
    let title = format!(
        " rinku node [{}] ",
        app.node_id.chars().take(8).collect::<String>()
    );
    
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan))
        .title(Span::styled(title, Style::default().fg(Color::Magenta).add_modifier(Modifier::BOLD)));

    frame.render_widget(block, area);
}

fn draw_tabs(frame: &mut Frame, app: &App, area: Rect) {
    let titles: Vec<Line> = Tab::all()
        .iter()
        .map(|t| {
            let style = if *t == app.current_tab {
                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            Line::from(Span::styled(t.title(), style))
        })
        .collect();

    let tabs = Tabs::new(titles)
        .select(Tab::all().iter().position(|t| *t == app.current_tab).unwrap_or(0))
        .divider(Span::raw(" | "));

    let inner = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(20), Constraint::Min(0)])
        .split(area);

    frame.render_widget(tabs, Rect::new(inner[1].x, inner[0].y + 1, inner[1].width, 1));
}

fn draw_dashboard(frame: &mut Frame, app: &App, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(8),
            Constraint::Length(8),
            Constraint::Min(0),
        ])
        .split(area);

    let node_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(25),
            Constraint::Percentage(25),
            Constraint::Percentage(25),
            Constraint::Percentage(25),
        ])
        .split(chunks[0]);

    let uptime = format_duration(app.node_stats.uptime_secs);
    draw_stat_box(frame, "Uptime", &uptime, Color::Green, node_chunks[0]);

    let cpu = format!("{:.1}%", app.node_stats.cpu_usage);
    draw_stat_box(frame, "CPU", &cpu, Color::Yellow, node_chunks[1]);

    let mem = format!(
        "{} / {} MB",
        app.node_stats.memory_used_mb, app.node_stats.memory_total_mb
    );
    draw_stat_box(frame, "System Memory", &mem, Color::Blue, node_chunks[2]);

    let proc_mem = format!("{} MB", app.node_stats.process_memory_mb);
    draw_stat_box(frame, "Node Memory", &proc_mem, Color::Magenta, node_chunks[3]);

    let network_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(25),
            Constraint::Percentage(25),
            Constraint::Percentage(25),
            Constraint::Percentage(25),
        ])
        .split(chunks[1]);

    let txs = format!("{}", app.network_stats.total_transactions);
    draw_stat_box(frame, "Transactions", &txs, Color::Cyan, network_chunks[0]);

    let finalized = format!(
        "{} ({:.0}%)",
        app.network_stats.finalized_count,
        if app.network_stats.total_transactions > 0 {
            app.network_stats.finalized_count as f64 / app.network_stats.total_transactions as f64 * 100.0
        } else {
            0.0
        }
    );
    draw_stat_box(frame, "Finalized", &finalized, Color::Green, network_chunks[1]);

    let checkpoint = format!("#{}", app.network_stats.checkpoint_height);
    draw_stat_box(frame, "Checkpoint", &checkpoint, Color::Yellow, network_chunks[2]);

    let gas = format!("{:.4} RKU", app.network_stats.gas_price);
    draw_stat_box(frame, "Gas Price", &gas, Color::Red, network_chunks[3]);

    let summary_block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray))
        .title(Span::styled(" Quick Stats ", Style::default().fg(Color::White)));

    let summary_text = vec![
        Line::from(vec![
            Span::styled("Tips: ", Style::default().fg(Color::DarkGray)),
            Span::styled(format!("{}", app.dag_stats.tip_count), Style::default().fg(Color::Cyan)),
            Span::raw("  |  "),
            Span::styled("Validators: ", Style::default().fg(Color::DarkGray)),
            Span::styled(format!("{}", app.validator_stats.total_validators), Style::default().fg(Color::Yellow)),
            Span::raw("  |  "),
            Span::styled("Total Staked: ", Style::default().fg(Color::DarkGray)),
            Span::styled(format!("{:.2} RKU", app.validator_stats.total_staked), Style::default().fg(Color::Green)),
            Span::raw("  |  "),
            Span::styled("Burned: ", Style::default().fg(Color::DarkGray)),
            Span::styled(format!("{:.4} RKU", app.network_stats.total_burned), Style::default().fg(Color::Red)),
        ]),
    ];

    let summary = Paragraph::new(summary_text)
        .block(summary_block)
        .style(Style::default());

    frame.render_widget(summary, chunks[2]);
}

fn draw_network(frame: &mut Frame, app: &App, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(area);

    let stats_block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan))
        .title(Span::styled(" Network Stats ", Style::default().fg(Color::Cyan)));

    let stats_items = vec![
        ListItem::new(Line::from(vec![
            Span::styled("Peer Count:      ", Style::default().fg(Color::DarkGray)),
            Span::styled(format!("{}", app.network_stats.peer_count), Style::default().fg(Color::White)),
        ])),
        ListItem::new(Line::from(vec![
            Span::styled("TPS:             ", Style::default().fg(Color::DarkGray)),
            Span::styled(format!("{:.2}", app.network_stats.tps), Style::default().fg(Color::Cyan)),
        ])),
        ListItem::new(Line::from(vec![
            Span::styled("Total Txs:       ", Style::default().fg(Color::DarkGray)),
            Span::styled(format!("{}", app.network_stats.total_transactions), Style::default().fg(Color::White)),
        ])),
        ListItem::new(Line::from(vec![
            Span::styled("Finalized:       ", Style::default().fg(Color::DarkGray)),
            Span::styled(format!("{}", app.network_stats.finalized_count), Style::default().fg(Color::Green)),
        ])),
        ListItem::new(Line::from(vec![
            Span::styled("Pending:         ", Style::default().fg(Color::DarkGray)),
            Span::styled(format!("{}", app.network_stats.pending_count), Style::default().fg(Color::Yellow)),
        ])),
        ListItem::new(Line::from(vec![
            Span::styled("Checkpoint:      ", Style::default().fg(Color::DarkGray)),
            Span::styled(format!("#{}", app.network_stats.checkpoint_height), Style::default().fg(Color::Magenta)),
        ])),
        ListItem::new(Line::from(vec![
            Span::styled("Gas Price:       ", Style::default().fg(Color::DarkGray)),
            Span::styled(format!("{:.6} RKU", app.network_stats.gas_price), Style::default().fg(Color::Red)),
        ])),
        ListItem::new(Line::from(vec![
            Span::styled("Total Burned:    ", Style::default().fg(Color::DarkGray)),
            Span::styled(format!("{:.6} RKU", app.network_stats.total_burned), Style::default().fg(Color::Red)),
        ])),
    ];

    let stats_list = List::new(stats_items).block(stats_block);
    frame.render_widget(stats_list, chunks[0]);

    let peers_block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Green))
        .title(Span::styled(" Connected Peers ", Style::default().fg(Color::Green)));

    let peer_items: Vec<ListItem> = if app.network_stats.peers.is_empty() {
        vec![ListItem::new(Span::styled(
            "No peers connected",
            Style::default().fg(Color::DarkGray),
        ))]
    } else {
        app.network_stats
            .peers
            .iter()
            .map(|p| {
                ListItem::new(Span::styled(
                    format!("  {}", p),
                    Style::default().fg(Color::White),
                ))
            })
            .collect()
    };

    let peers_list = List::new(peer_items).block(peers_block);
    frame.render_widget(peers_list, chunks[1]);
}

fn draw_validator(frame: &mut Frame, app: &App, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(12), Constraint::Min(0)])
        .split(area);

    let validator_block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Yellow))
        .title(Span::styled(" Validator Status ", Style::default().fg(Color::Yellow)));

    let status_color = if app.validator_stats.is_validator {
        Color::Green
    } else {
        Color::DarkGray
    };

    let status_text = if app.validator_stats.is_validator {
        "ACTIVE"
    } else {
        "NOT A VALIDATOR"
    };

    let items = vec![
        ListItem::new(Line::from(vec![
            Span::styled("Status:          ", Style::default().fg(Color::DarkGray)),
            Span::styled(status_text, Style::default().fg(status_color).add_modifier(Modifier::BOLD)),
        ])),
        ListItem::new(Line::from(vec![
            Span::styled("Address:         ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                app.validator_stats.address.clone().unwrap_or_else(|| "N/A".to_string()),
                Style::default().fg(Color::Cyan),
            ),
        ])),
        ListItem::new(Line::from(vec![
            Span::styled("Stake:           ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                format!("{:.2} RKU", app.validator_stats.stake_amount),
                Style::default().fg(Color::Green),
            ),
        ])),
        ListItem::new(Line::from(vec![
            Span::styled("Pending Rewards: ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                format!("{:.6} RKU", app.validator_stats.pending_rewards),
                Style::default().fg(Color::Yellow),
            ),
        ])),
        ListItem::new(Line::from(vec![
            Span::styled("Unbonding:       ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                format!("{:.2} RKU", app.validator_stats.unbonding_amount),
                Style::default().fg(Color::Red),
            ),
        ])),
        ListItem::new(Line::from("")),
        ListItem::new(Line::from(vec![
            Span::styled("Network Total Validators: ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                format!("{}", app.validator_stats.total_validators),
                Style::default().fg(Color::Magenta),
            ),
        ])),
        ListItem::new(Line::from(vec![
            Span::styled("Network Total Staked:     ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                format!("{:.2} RKU", app.validator_stats.total_staked),
                Style::default().fg(Color::Green),
            ),
        ])),
    ];

    let list = List::new(items).block(validator_block);
    frame.render_widget(list, chunks[0]);

    let help_block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray))
        .title(Span::styled(" Validator Actions ", Style::default().fg(Color::White)));

    let help_text = vec![
        Line::from(""),
        Line::from(Span::styled(
            "  To become a validator, stake RKU tokens using the explorer or CLI.",
            Style::default().fg(Color::DarkGray),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "  Minimum stake: 1000 RKU",
            Style::default().fg(Color::Yellow),
        )),
        Line::from(Span::styled(
            "  Unbonding period: 7 days",
            Style::default().fg(Color::Yellow),
        )),
    ];

    let help = Paragraph::new(help_text).block(help_block);
    frame.render_widget(help, chunks[1]);
}

fn draw_dag(frame: &mut Frame, app: &App, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
        .split(area);

    let tips_block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Magenta))
        .title(Span::styled(
            format!(" DAG Tips ({}) ", app.dag_stats.tip_count),
            Style::default().fg(Color::Magenta),
        ));

    let tip_items: Vec<ListItem> = app
        .dag_stats
        .tips
        .iter()
        .enumerate()
        .map(|(i, t)| {
            ListItem::new(Line::from(vec![
                Span::styled(format!("{:2}. ", i + 1), Style::default().fg(Color::DarkGray)),
                Span::styled(format!("{}...", t), Style::default().fg(Color::Cyan)),
            ]))
        })
        .collect();

    let tips_list = List::new(tip_items).block(tips_block);
    frame.render_widget(tips_list, chunks[0]);

    let txs_block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan))
        .title(Span::styled(" Recent Transactions ", Style::default().fg(Color::Cyan)));

    let header = Row::new(vec!["Hash", "From", "To", "Amount", "Status"])
        .style(Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD))
        .bottom_margin(1);

    let rows: Vec<Row> = app
        .dag_stats
        .recent_txs
        .iter()
        .map(|tx| {
            let status_style = if tx.finalized {
                Style::default().fg(Color::Green)
            } else {
                Style::default().fg(Color::Yellow)
            };
            Row::new(vec![
                format!("{}...", tx.hash),
                format!("{}...", tx.from),
                format!("{}...", tx.to),
                format!("{:.2}", tx.amount),
                if tx.finalized { "finalized" } else { "pending" }.to_string(),
            ])
            .style(status_style)
        })
        .collect();

    let table = Table::new(
        rows,
        [
            Constraint::Length(14),
            Constraint::Length(10),
            Constraint::Length(10),
            Constraint::Length(12),
            Constraint::Length(10),
        ],
    )
    .header(header)
    .block(txs_block);

    frame.render_widget(table, chunks[1]);
}

fn draw_logs(frame: &mut Frame, app: &App, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray))
        .title(Span::styled(" Logs ", Style::default().fg(Color::White)));

    let max_display = (area.height as usize).saturating_sub(2);
    let start = app.scroll_offset.min(app.logs.len().saturating_sub(max_display));
    let visible_logs: Vec<ListItem> = app
        .logs
        .iter()
        .skip(start)
        .take(max_display)
        .map(|l| ListItem::new(Span::styled(l.clone(), Style::default().fg(Color::White))))
        .collect();

    let list = List::new(visible_logs).block(block);
    frame.render_widget(list, area);
}

fn draw_footer(frame: &mut Frame, _app: &App, area: Rect) {
    let help = vec![
        Span::styled(" Tab ", Style::default().fg(Color::Black).bg(Color::Cyan)),
        Span::raw(" Switch view  "),
        Span::styled(" ↑↓ ", Style::default().fg(Color::Black).bg(Color::Cyan)),
        Span::raw(" Scroll  "),
        Span::styled(" q ", Style::default().fg(Color::Black).bg(Color::Red)),
        Span::raw(" Quit  "),
        Span::styled(" ? ", Style::default().fg(Color::Black).bg(Color::Yellow)),
        Span::raw(" Help "),
    ];

    let footer = Paragraph::new(Line::from(help))
        .style(Style::default().fg(Color::DarkGray))
        .block(Block::default().borders(Borders::ALL).border_style(Style::default().fg(Color::DarkGray)));

    frame.render_widget(footer, area);
}

fn draw_stat_box(frame: &mut Frame, title: &str, value: &str, color: Color, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(color))
        .title(Span::styled(
            format!(" {} ", title),
            Style::default().fg(color),
        ));

    let text = Paragraph::new(Line::from(Span::styled(
        value,
        Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
    )))
    .block(block)
    .style(Style::default())
    .alignment(ratatui::layout::Alignment::Center);

    frame.render_widget(text, area);
}

fn format_duration(secs: u64) -> String {
    let days = secs / 86400;
    let hours = (secs % 86400) / 3600;
    let mins = (secs % 3600) / 60;
    let secs = secs % 60;

    if days > 0 {
        format!("{}d {}h {}m", days, hours, mins)
    } else if hours > 0 {
        format!("{}h {}m {}s", hours, mins, secs)
    } else if mins > 0 {
        format!("{}m {}s", mins, secs)
    } else {
        format!("{}s", secs)
    }
}
