use std::collections::HashMap;

use color_eyre::eyre::WrapErr;
use crossterm::event::{
    Event as CrosstermEvent, KeyCode, KeyModifiers, MouseEventKind,
};
use futures::{FutureExt, StreamExt};
use ratatui::{
    DefaultTerminal, Frame,
    layout::{Alignment, Constraint, Direction, Layout},
    style::{Color, Style},
    text::{Line, Text},
    widgets::{Block, Gauge},
};
use tui_logger::{TuiLoggerWidget, TuiWidgetEvent, TuiWidgetState};

use crate::{
    event::{AppEvent, Event},
    proxy::ProxyType,
    utils::is_docker,
};

const FPS: f64 = 30.0;

pub(crate) struct Tui {
    terminal: DefaultTerminal,
}

impl super::UI for Tui {
    fn new() -> color_eyre::Result<Self> {
        tui_logger::init_logger(log::LevelFilter::Info)
            .wrap_err("failed to initialize logger")?;
        tui_logger::set_default_level(log::LevelFilter::Trace);
        Ok(Self { terminal: ratatui::init() })
    }

    fn set_log_level(log_level: log::LevelFilter) {
        log::set_max_level(log_level);
    }

    async fn run(
        mut self,
        tx: tokio::sync::mpsc::UnboundedSender<Event>,
        mut rx: tokio::sync::mpsc::UnboundedReceiver<Event>,
    ) -> color_eyre::Result<()> {
        let tick_task = tokio::spawn(tick_event_listener(tx.clone()));
        let crossterm_task = tokio::spawn(crossterm_event_listener(tx));
        let mut app_state = AppState::new();
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
        tick_task.await.wrap_err("failed to spawn tui tick task")??;
        crossterm_task
            .await
            .wrap_err("failed to spawn tui crossterm task")??;
        Ok(())
    }
}

impl Drop for Tui {
    fn drop(&mut self) {
        ratatui::restore();
    }
}

#[derive(Default)]
pub(crate) enum AppMode {
    #[default]
    Running,
    /// Wait for the user confirmation to close the UI
    Done,
    /// Close the UI
    Quit,
}

#[derive(Default)]
pub(crate) struct AppState {
    pub(crate) mode: AppMode,

    pub(crate) geodb_total: u64,
    pub(crate) geodb_downloaded: usize,

    pub(crate) sources_total: HashMap<ProxyType, usize>,
    pub(crate) sources_scraped: HashMap<ProxyType, usize>,

    pub(crate) proxies_total: HashMap<ProxyType, usize>,
    pub(crate) proxies_checked: HashMap<ProxyType, usize>,
    pub(crate) proxies_working: HashMap<ProxyType, usize>,
}

impl AppState {
    pub(crate) fn new() -> Self {
        AppState::default()
    }
}

async fn tick_event_listener(
    tx: tokio::sync::mpsc::UnboundedSender<Event>,
) -> color_eyre::Result<()> {
    let mut tick =
        tokio::time::interval(tokio::time::Duration::from_secs_f64(1.0 / FPS));
    loop {
        let closed = tx.closed().fuse();
        let ticked = tick.tick().fuse();
        tokio::select! {
            biased;
            () = closed => {
                break Ok(())
            },
            _ = ticked =>{
                if tx.send(Event::Tick).is_err() {
                    break Ok(());
                }
            }
        }
    }
}

async fn crossterm_event_listener(
    tx: tokio::sync::mpsc::UnboundedSender<Event>,
) -> color_eyre::Result<()> {
    let mut reader = crossterm::event::EventStream::new();
    loop {
        let closed = tx.closed().fuse();
        let event = reader.next().fuse();
        tokio::select! {
            biased;
            () = closed => {
                break Ok(());
            },
            maybe = event => {
                match maybe {
                    Some(Ok(event)) => {
                        if tx.send(Event::Crossterm(event)).is_err() {
                            break Ok(());
                        }
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

#[allow(clippy::cast_possible_truncation)]
#[allow(clippy::cast_precision_loss)]
#[allow(clippy::too_many_lines)]
fn draw(f: &mut Frame, state: &AppState, logger_state: &TuiWidgetState) {
    let outer_block = Block::default()
        .title("https://github.com/monosans/proxy-scraper-checker")
        .title_alignment(Alignment::Center);
    f.render_widget(outer_block.clone(), f.area());
    let outer_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints(vec![
            // Logs
            Constraint::Fill(1),
            // GeoDB download
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

    f.render_widget(
        Gauge::default()
            .block(Block::bordered().title("GeoDB download"))
            .ratio(if state.geodb_total == 0 {
                1.0
            } else {
                (state.geodb_downloaded as f64) / (state.geodb_total as f64)
            }),
        outer_layout[1],
    );

    let proxies_layout = Layout::default()
        .direction(Direction::Horizontal)
        .constraints(
            state
                .sources_total
                .keys()
                .map(|_| Constraint::Fill(1))
                .collect::<Vec<_>>(),
        )
        .split(outer_layout[2]);

    let mut proxy_types: Vec<_> = state.sources_total.keys().collect();
    proxy_types.sort();

    for (i, proxy_type) in proxy_types.iter().enumerate() {
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
                .ratio(if sources_total == 0 {
                    0.0
                } else {
                    sources_scraped as f64 / sources_total as f64
                })
                .block(Block::bordered().title("Scraping sources"))
                .label(format!("{sources_scraped}/{sources_total}")),
            layout[0],
        );

        let proxies_checked =
            state.proxies_checked.get(proxy_type).copied().unwrap_or_default();
        let proxies_working =
            state.proxies_working.get(proxy_type).copied().unwrap_or_default();
        let proxies_total =
            state.proxies_total.get(proxy_type).copied().unwrap_or_default();

        f.render_widget(
            Gauge::default()
                .ratio(if proxies_total == 0 {
                    0.0
                } else {
                    proxies_checked as f64 / proxies_total as f64
                })
                .block(Block::bordered().title("Checking proxies"))
                .label(format!("{proxies_checked}/{proxies_total}")),
            layout[1],
        );

        f.render_widget(
            Gauge::default()
                .ratio(if proxies_total == 0 {
                    0.0
                } else {
                    proxies_checked as f64 / proxies_total as f64
                })
                .block(
                    Block::bordered()
                        .title("Working proxies / checked proxies"),
                )
                .label(format!(
                    "{}/{} ({:.1}%)",
                    proxies_working,
                    proxies_checked,
                    if proxies_working != 0 {
                        (proxies_working as f64 / proxies_checked as f64)
                            * 100.0
                    } else {
                        0.0
                    }
                )),
            layout[2],
        );
    }

    let mut lines = vec![
        Line::from("Up/PageUp/k - scroll logs up"),
        Line::from("Down/PageDown/j - scroll logs down"),
    ];
    if let AppMode::Done = state.mode {
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
                AppEvent::GeoDbTotal(bytes) => {
                    state.geodb_total = bytes.unwrap_or_default();
                }
                AppEvent::GeoDbDownloaded(bytes) => {
                    state.geodb_downloaded += bytes;
                }
                AppEvent::SourcesTotal(proxy_type, amount) => {
                    state.sources_total.insert(proxy_type, amount);
                }
                AppEvent::SourceScraped(proxy_type) => {
                    *state.sources_scraped.entry(proxy_type).or_default() += 1;
                }
                AppEvent::TotalProxies(proxy_type, amount) => {
                    state.proxies_total.insert(proxy_type, amount);
                }
                AppEvent::ProxyChecked(proxy_type) => {
                    *state.proxies_checked.entry(proxy_type).or_default() += 1;
                }
                AppEvent::ProxyWorking(proxy_type) => {
                    *state.proxies_working.entry(proxy_type).or_default() += 1;
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
