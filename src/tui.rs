#![expect(
    clippy::indexing_slicing,
    clippy::missing_asserts_for_indexing,
    clippy::wildcard_enum_match_arm
)]

use std::time::Duration;

use crossterm::event::{
    Event as CrosstermEvent, KeyCode, KeyModifiers, MouseEventKind,
};
use futures::StreamExt as _;
use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Direction, Layout},
    style::{Color, Style},
    text::{Line, Text},
    widgets::{Block, Gauge},
};
use tui_logger::{TuiLoggerWidget, TuiWidgetEvent, TuiWidgetState};

use crate::{
    HashMap,
    event::{AppEvent, Event},
    ipdb,
    proxy::ProxyType,
};

const FPS: f64 = 30.0;

pub struct RatatuiRestoreGuard;
impl Drop for RatatuiRestoreGuard {
    fn drop(&mut self) {
        ratatui::restore();
    }
}

pub async fn run(
    mut terminal: ratatui::DefaultTerminal,
    token: tokio_util::sync::CancellationToken,
    tx: tokio::sync::mpsc::UnboundedSender<Event>,
    mut rx: tokio::sync::mpsc::UnboundedReceiver<Event>,
) -> crate::Result<()> {
    tokio::spawn(tick_event_listener(tx.clone()));
    tokio::spawn(crossterm_event_listener(tx));

    let mut app_state = AppState::default();
    let logger_state = TuiWidgetState::default();

    while !matches!(app_state.mode, AppMode::Quit) {
        if let Some(event) = rx.recv().await {
            if handle_event(event, &mut app_state, &token, &logger_state) {
                terminal
                    .draw(|frame| draw(frame, &app_state, &logger_state))?;
            }
        } else {
            break;
        }
    }
    Ok(())
}

#[derive(Default)]
pub enum AppMode {
    #[default]
    Running,
    Done,
    Quit,
}

impl AppMode {
    pub const fn next(&self) -> Self {
        match self {
            Self::Running => Self::Done,
            Self::Done | Self::Quit => Self::Quit,
        }
    }
}

#[derive(Default)]
pub struct AppState {
    pub mode: AppMode,

    pub asn_db_total: u64,
    pub asn_db_downloaded: usize,

    pub geo_db_total: u64,
    pub geo_db_downloaded: usize,

    pub sources_total: HashMap<ProxyType, usize>,
    pub sources_scraped: HashMap<ProxyType, usize>,

    pub proxies_total: HashMap<ProxyType, usize>,
    pub proxies_checked: HashMap<ProxyType, usize>,
    pub proxies_working: HashMap<ProxyType, usize>,
}

async fn tick_event_listener(tx: tokio::sync::mpsc::UnboundedSender<Event>) {
    let mut tick = tokio::time::interval(Duration::from_secs_f64(1.0 / FPS));
    loop {
        tokio::select! {
            biased;
            () = tx.closed() => {
                break;
            },
            _ = tick.tick() => {
                if tx.send(Event::Tick).is_err() {
                    break;
                }
            },
        }
    }
}

async fn crossterm_event_listener(
    tx: tokio::sync::mpsc::UnboundedSender<Event>,
) {
    let mut reader = crossterm::event::EventStream::new();
    loop {
        tokio::select! {
            biased;
            () = tx.closed() => {
                break;
            },
            maybe = reader.next() => {
                match maybe {
                    Some(Ok(event)) => {
                        if tx.send(Event::Crossterm(event)).is_err() {
                            break;
                        }
                    },
                    Some(Err(_)) => {},
                    None => {
                        break;
                    }
                }
            },
        }
    }
}

fn draw(f: &mut Frame<'_>, state: &AppState, logger_state: &TuiWidgetState) {
    let outer_block = Block::default()
        .title("https://github.com/monosans/proxy-scraper-checker")
        .title_alignment(Alignment::Center);
    f.render_widget(outer_block.clone(), f.area());
    let outer_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            // Logs
            Constraint::Fill(1),
            // IP database download
            Constraint::Length(3),
            // Scraping and checking
            Constraint::Length(1 + (3 * 3) + 1),
            // Hotkeys
            Constraint::Length(4),
        ])
        .split(outer_block.inner(f.area()));
    drop(outer_block);

    f.render_widget(
        TuiLoggerWidget::default()
            .state(logger_state)
            .block(Block::bordered().title("Logs"))
            .output_file(false)
            .output_line(false)
            .style_trace(Style::default().fg(Color::Magenta))
            .style_debug(Style::default().fg(Color::Green))
            .style_info(Style::default().fg(Color::Cyan))
            .style_warn(Style::default().fg(Color::Yellow))
            .style_error(Style::default().fg(Color::Red)),
        outer_layout[0],
    );

    let ipdb_layout = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Fill(1); 2])
        .split(outer_layout[1]);
    f.render_widget(
        Gauge::default()
            .block(Block::bordered().title("ASN database download"))
            .ratio({
                if state.asn_db_total == 0 {
                    1.0
                } else {
                    (state.asn_db_downloaded as f64)
                        / (state.asn_db_total as f64)
                }
            }),
        ipdb_layout[0],
    );
    f.render_widget(
        Gauge::default()
            .block(Block::bordered().title("Geolocation database download"))
            .ratio({
                if state.geo_db_total == 0 {
                    1.0
                } else {
                    (state.geo_db_downloaded as f64)
                        / (state.geo_db_total as f64)
                }
            }),
        ipdb_layout[1],
    );
    drop(ipdb_layout);

    let mut proxy_types: Vec<_> = state.sources_total.keys().collect();
    proxy_types.sort_unstable();

    let proxies_layout = Layout::default()
        .direction(Direction::Horizontal)
        .constraints(proxy_types.iter().map(|_| Constraint::Fill(1)))
        .split(outer_layout[2]);

    for (i, proxy_type) in proxy_types.into_iter().enumerate() {
        let block = Block::bordered().title(proxy_type.as_str().to_uppercase());
        f.render_widget(block.clone(), proxies_layout[i]);

        let layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Fill(1); 3])
            .split(block.inner(proxies_layout[i]));
        drop(block);

        let sources_scraped =
            state.sources_scraped.get(proxy_type).copied().unwrap_or_default();
        let sources_total =
            state.sources_total.get(proxy_type).copied().unwrap_or_default();

        f.render_widget(
            Gauge::default()
                .ratio({
                    if sources_total == 0 {
                        1.0
                    } else {
                        (sources_scraped as f64) / (sources_total as f64)
                    }
                })
                .block(Block::bordered().title("Scraping sources"))
                .label(format!("{sources_scraped}/{sources_total}")),
            layout[0],
        );

        let proxies_total =
            state.proxies_total.get(proxy_type).copied().unwrap_or_default();
        let proxies_checked =
            state.proxies_checked.get(proxy_type).copied().unwrap_or_default();
        f.render_widget(
            Gauge::default()
                .ratio({
                    if proxies_total == 0 {
                        1.0
                    } else {
                        (proxies_checked as f64) / (proxies_total as f64)
                    }
                })
                .block(Block::bordered().title("Checking proxies"))
                .label(format!("{proxies_checked}/{proxies_total}")),
            layout[1],
        );

        let working_proxies_block = Block::bordered().title("Working proxies");
        f.render_widget(working_proxies_block.clone(), layout[2]);

        let proxies_working =
            state.proxies_working.get(proxy_type).copied().unwrap_or_default();
        f.render_widget(
            Line::from(format!("{} ({:.1}%)", proxies_working, {
                if proxies_checked == 0 {
                    0.0_f64
                } else {
                    (proxies_working as f64) / (proxies_checked as f64)
                        * 100.0_f64
                }
            }))
            .alignment(Alignment::Center),
            working_proxies_block.inner(layout[2]),
        );
    }

    drop(proxies_layout);

    let running = matches!(state.mode, AppMode::Running);
    let mut lines = Vec::with_capacity(if running { 4 } else { 3 });
    lines.push(Line::from("Up / PageUp / k - scroll logs up"));
    lines.push(Line::from("Down / PageDown / j - scroll logs down"));
    if running {
        lines.push(
            Line::from("ESC / q - stop")
                .style(Style::default().fg(Color::Yellow)),
        );
    }
    lines.push(
        Line::from(if running {
            "Ctrl-C - quit"
        } else {
            "ESC / q / Ctrl-C - quit"
        })
        .style(Style::default().fg(Color::Red)),
    );

    f.render_widget(Text::from(lines).centered(), outer_layout[3]);
}

fn handle_event(
    event: Event,
    state: &mut AppState,
    token: &tokio_util::sync::CancellationToken,
    logger_state: &TuiWidgetState,
) -> bool {
    match event {
        Event::Tick => true,
        Event::Crossterm(crossterm_event) => {
            match crossterm_event {
                CrosstermEvent::Key(key_event) => match key_event.code {
                    KeyCode::Esc | KeyCode::Char('q' | 'Q') => {
                        state.mode = state.mode.next();
                        token.cancel();
                    }
                    KeyCode::Char('c' | 'C')
                        if key_event.modifiers == KeyModifiers::CONTROL =>
                    {
                        state.mode = AppMode::Quit;
                        token.cancel();
                    }
                    KeyCode::Up | KeyCode::PageUp | KeyCode::Char('k') => {
                        logger_state.transition(TuiWidgetEvent::PrevPageKey);
                    }
                    KeyCode::Down | KeyCode::PageDown | KeyCode::Char('j') => {
                        logger_state.transition(TuiWidgetEvent::NextPageKey);
                    }
                    _ => {}
                },
                CrosstermEvent::Mouse(mouse_event) => match mouse_event.kind {
                    MouseEventKind::ScrollUp => {
                        logger_state.transition(TuiWidgetEvent::PrevPageKey);
                    }
                    MouseEventKind::ScrollDown => {
                        logger_state.transition(TuiWidgetEvent::NextPageKey);
                    }
                    _ => {}
                },
                _ => {}
            }
            false
        }
        Event::App(app_event) => {
            match app_event {
                AppEvent::IpDbTotal(ipdb::DbType::Asn, bytes) => {
                    state.asn_db_total = bytes.unwrap_or_default();
                }
                AppEvent::IpDbTotal(ipdb::DbType::Geo, bytes) => {
                    state.geo_db_total = bytes.unwrap_or_default();
                }
                AppEvent::IpDbDownloaded(ipdb::DbType::Asn, bytes) => {
                    state.asn_db_downloaded =
                        state.asn_db_downloaded.saturating_add(bytes);
                }
                AppEvent::IpDbDownloaded(ipdb::DbType::Geo, bytes) => {
                    state.geo_db_downloaded =
                        state.geo_db_downloaded.saturating_add(bytes);
                }
                AppEvent::SourcesTotal(proxy_type, amount) => {
                    state.sources_total.insert(proxy_type, amount);
                }
                AppEvent::SourceScraped(proxy_type) => {
                    state
                        .sources_scraped
                        .entry(proxy_type)
                        .and_modify(|c| *c = c.saturating_add(1))
                        .or_insert(1);
                }
                AppEvent::TotalProxies(proxy_type, amount) => {
                    state.proxies_total.insert(proxy_type, amount);
                }
                AppEvent::ProxyChecked(proxy_type) => {
                    state
                        .proxies_checked
                        .entry(proxy_type)
                        .and_modify(|c| *c = c.saturating_add(1))
                        .or_insert(1);
                }
                AppEvent::ProxyWorking(proxy_type) => {
                    state
                        .proxies_working
                        .entry(proxy_type)
                        .and_modify(|c| *c = c.saturating_add(1))
                        .or_insert(1);
                }
                AppEvent::Done => {
                    if matches!(state.mode, AppMode::Running) {
                        state.mode = AppMode::Done;
                    }
                }
                AppEvent::Quit => {
                    state.mode = AppMode::Quit;
                }
            }
            false
        }
    }
}
