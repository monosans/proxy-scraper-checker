#![expect(
    clippy::indexing_slicing,
    clippy::missing_asserts_for_indexing,
    clippy::wildcard_enum_match_arm
)]

use std::collections::HashMap;

use color_eyre::eyre::WrapErr as _;
use crossterm::event::{
    Event as CrosstermEvent, KeyCode, KeyModifiers, MouseEventKind,
};
use futures::StreamExt as _;
use ratatui::{
    DefaultTerminal, Frame,
    layout::{Alignment, Constraint, Direction, Layout},
    style::{Color, Style},
    text::{Line, Text},
    widgets::{Block, Gauge},
};
use tracing_subscriber::{
    layer::SubscriberExt as _, util::SubscriberInitExt as _,
};
use tui_logger::{TuiLoggerWidget, TuiWidgetEvent, TuiWidgetState};

use crate::{
    event::{AppEvent, Event},
    ipdb,
    proxy::ProxyType,
    utils::is_docker,
};

const FPS: f64 = 30.0;

pub struct Tui {
    terminal: DefaultTerminal,
}

impl Tui {
    pub fn new(
        filter: tracing_subscriber::filter::Targets,
    ) -> color_eyre::Result<Self> {
        tui_logger::init_logger(tui_logger::LevelFilter::Trace)
            .wrap_err("failed to initialize tui_logger")?;

        tracing_subscriber::registry()
            .with(filter)
            .with(tui_logger::TuiTracingSubscriberLayer)
            .init();

        Ok(Self {
            terminal: ratatui::try_init()
                .wrap_err("failed to initialize ratatui")?,
        })
    }

    pub async fn run(
        mut self,
        tx: tokio::sync::mpsc::UnboundedSender<Event>,
        mut rx: tokio::sync::mpsc::UnboundedReceiver<Event>,
    ) -> color_eyre::Result<()> {
        let mut join_set = tokio::task::JoinSet::new();
        join_set.spawn(tick_event_listener(tx.clone()));
        join_set.spawn(crossterm_event_listener(tx));

        let mut app_state = AppState::default();
        let logger_state = TuiWidgetState::default();
        while !matches!(app_state.mode, AppMode::Quit) {
            if let Some(event) = rx.recv().await {
                if handle_event(event, &mut app_state, &logger_state).await {
                    self.terminal
                        .draw(|frame| draw(frame, &app_state, &logger_state))
                        .wrap_err("failed to draw tui")?;
                }
            } else {
                break;
            }
        }
        drop(rx);
        while let Some(task) = join_set.join_next().await {
            task.wrap_err("failed to join event listener task")?
                .wrap_err("event listener task failed")?;
        }
        Ok(())
    }
}

impl Drop for Tui {
    fn drop(&mut self) {
        ratatui::restore();
    }
}

#[derive(Default)]
pub enum AppMode {
    #[default]
    Running,
    /// Wait for the user confirmation to close the UI
    Done,
    /// Close the UI
    Quit,
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

async fn tick_event_listener(
    tx: tokio::sync::mpsc::UnboundedSender<Event>,
) -> Result<(), tokio::sync::mpsc::error::SendError<Event>> {
    let mut tick =
        tokio::time::interval(tokio::time::Duration::from_secs_f64(1.0 / FPS));
    #[expect(clippy::integer_division_remainder_used)]
    loop {
        tokio::select! {
            biased;
            () = tx.closed() => {
                break Ok(());
            },
            _ = tick.tick() =>{
                tx.send(Event::Tick)?;
            }
        }
    }
}

async fn crossterm_event_listener(
    tx: tokio::sync::mpsc::UnboundedSender<Event>,
) -> Result<(), tokio::sync::mpsc::error::SendError<Event>> {
    let mut reader = crossterm::event::EventStream::new();
    #[expect(clippy::integer_division_remainder_used)]
    loop {
        tokio::select! {
            biased;
            () = tx.closed() => {
                break Ok(());
            },
            maybe = reader.next() => {
                match maybe {
                    Some(Ok(event)) => {
                        tx.send(Event::Crossterm(event))?;
                    },
                    Some(Err(_)) => {},
                    None => {
                        break Ok(());
                    }
                }
            }
        }
    }
}

fn draw(f: &mut Frame, state: &AppState, logger_state: &TuiWidgetState) {
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
            Constraint::Length(3),
        ])
        .split(outer_block.inner(f.area()));

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

    let proxies_layout = Layout::default()
        .direction(Direction::Horizontal)
        .constraints(state.sources_total.keys().map(|_| Constraint::Fill(1)))
        .split(outer_layout[2]);

    let mut proxy_types: Vec<_> = state.sources_total.keys().collect();
    proxy_types.sort();

    for (i, proxy_type) in proxy_types.into_iter().enumerate() {
        let block =
            Block::bordered().title(proxy_type.to_string().to_uppercase());
        f.render_widget(block.clone(), proxies_layout[i]);

        let layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Fill(1); 3])
            .split(block.inner(proxies_layout[i]));

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

    let done = matches!(state.mode, AppMode::Done);
    let mut lines = Vec::with_capacity(usize::from(done).saturating_add(2));
    lines.push(Line::from("Up/PageUp/k - scroll logs up"));
    lines.push(Line::from("Down/PageDown/j - scroll logs down"));
    if done {
        lines.push(
            Line::from("Enter/ESC/q/Ctrl-C - exit")
                .style(Style::default().fg(Color::Red)),
        );
    }
    f.render_widget(Text::from(lines).centered(), outer_layout[3]);
}

async fn is_interactive() -> bool {
    !is_docker().await
}

async fn handle_event(
    event: Event,
    state: &mut AppState,
    logger_state: &TuiWidgetState,
) -> bool {
    match event {
        Event::Tick => true,
        Event::Crossterm(crossterm_event) => {
            match crossterm_event {
                CrosstermEvent::Key(key_event) => match key_event.code {
                    KeyCode::Enter
                    | KeyCode::Esc
                    | KeyCode::Char('q' | 'Q')
                        if matches!(state.mode, AppMode::Done) =>
                    {
                        state.mode = AppMode::Quit;
                    }
                    KeyCode::Char('c' | 'C')
                        if key_event.modifiers == KeyModifiers::CONTROL
                            && matches!(state.mode, AppMode::Done) =>
                    {
                        state.mode = AppMode::Quit;
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
                    state.mode = if is_interactive().await {
                        AppMode::Done
                    } else {
                        AppMode::Quit
                    };
                }
            }
            false
        }
    }
}
