#![expect(clippy::wildcard_enum_match_arm)]

use std::{sync::Arc, time::Duration};

use crossterm::event::{Event as CrosstermEvent, KeyCode, KeyModifiers};
use futures::StreamExt as _;
use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Color, Style},
    text::{Line, Text},
    widgets::{Block, Gauge},
};
use tui_logger::{TuiLoggerWidget, TuiWidgetEvent, TuiWidgetState};

use crate::{
    config::Config,
    event::{AppEvent, Event},
    ipdb,
    proxy::PerProxyType,
};

const MIN_COLUMN_WIDTH: u16 = 24;
const BORDERS: u16 = 2;
const GAUGE_HEIGHT: u16 = 3;
const GAUGES_PER_COLUMN: u16 = 3;
const COLUMNS_HEIGHT: u16 = BORDERS + GAUGES_PER_COLUMN * GAUGE_HEIGHT;
const HOTKEYS_HEIGHT: u16 = 4;
const MAX_INPUT_ERRORS: u8 = 8;
const URL: &str = "https://github.com/monosans/proxy-scraper-checker";

pub struct RatatuiRestoreGuard;

impl Drop for RatatuiRestoreGuard {
    fn drop(&mut self) {
        ratatui::restore();
    }
}

#[derive(Default)]
enum AppMode {
    #[default]
    Running,
    Done,
    Quit,
}

impl AppMode {
    const fn next(&self) -> Self {
        match self {
            Self::Running => Self::Done,
            Self::Done | Self::Quit => Self::Quit,
        }
    }
}

#[derive(Clone, Copy, Default)]
struct DbProgress {
    finished_ok: Option<bool>,
    downloaded: usize,
    total: usize,
}

#[derive(Clone, Copy, Default)]
struct ProtocolProgress {
    sources_total: usize,
    sources_scraped: usize,
    proxies_total: usize,
    proxies_checked: usize,
    proxies_working: usize,
}

#[derive(Default)]
struct AppState {
    mode: AppMode,
    input_lost: bool,
    asn_db: DbProgress,
    geo_db: DbProgress,
    protocols: PerProxyType<ProtocolProgress>,
}

impl AppState {
    const fn db_mut(&mut self, db_type: ipdb::DbType) -> &mut DbProgress {
        match db_type {
            ipdb::DbType::Asn => &mut self.asn_db,
            ipdb::DbType::Geo => &mut self.geo_db,
        }
    }
}

pub async fn run(
    mut terminal: ratatui::DefaultTerminal,
    config: Arc<Config>,
    token: tokio_util::sync::CancellationToken,
    tx: tokio::sync::mpsc::UnboundedSender<Event>,
    mut rx: tokio::sync::mpsc::UnboundedReceiver<Event>,
) -> crate::Result<()> {
    tokio::spawn(tick_event_listener(tx.clone()));
    tokio::spawn(crossterm_event_listener(tx));

    let mut app_state = AppState::default();
    let logger_state = TuiWidgetState::default();

    terminal.draw(|frame| draw(frame, &config, &app_state, &logger_state))?;

    while !matches!(app_state.mode, AppMode::Quit) {
        let Some(event) = rx.recv().await else {
            break;
        };
        let mut redraw =
            handle_event(event, &mut app_state, &token, &logger_state);

        while !matches!(app_state.mode, AppMode::Quit)
            && let Ok(event) = rx.try_recv()
        {
            redraw |=
                handle_event(event, &mut app_state, &token, &logger_state);
        }

        if redraw {
            terminal.draw(|frame| {
                draw(frame, &config, &app_state, &logger_state);
            })?;
        }
    }
    Ok(())
}

async fn tick_event_listener(tx: tokio::sync::mpsc::UnboundedSender<Event>) {
    let mut tick = tokio::time::interval(Duration::from_millis(100));
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
    let mut consecutive_errors: u8 = 0;

    loop {
        tokio::select! {
            biased;
            () = tx.closed() => return,
            event = reader.next() => {
                match event {
                    Some(Ok(event)) => {
                        consecutive_errors = 0;
                        if tx.send(Event::Crossterm(event)).is_err() {
                            return;
                        }
                    }
                    Some(Err(e)) => {
                        tracing::debug!("Failed to read terminal input: {e}");
                        consecutive_errors = consecutive_errors.saturating_add(1);
                        // A terminal that keeps failing to deliver input would
                        // otherwise spin this loop; the run itself carries on.
                        if consecutive_errors >= MAX_INPUT_ERRORS {
                            break;
                        }
                    }
                    None => break,
                }
            },
        }
    }

    tracing::warn!(
        "Terminal input was lost, so the hotkeys no longer work. The program \
         will exit when the run finishes."
    );
    drop(tx.send(Event::App(AppEvent::InputLost)));
}

fn ratio(done: usize, total: usize) -> f64 {
    if total == 0 {
        return 0.0_f64;
    }
    ((done as f64) / (total as f64)).clamp(0.0_f64, 1.0_f64)
}

fn draw(
    f: &mut Frame<'_>,
    config: &Config,
    state: &AppState,
    logger_state: &TuiWidgetState,
) {
    let area = f.area();

    let columns = u16::try_from(config.scraping.enabled_protocols.len())
        .unwrap_or(u16::MAX);
    let stacked = area.width < MIN_COLUMN_WIDTH.saturating_mul(columns);

    let ipdb_shown = config.asn_enabled() || config.geolocation_enabled();

    let mut outer_block = Block::new();
    if usize::from(area.width) >= URL.len() {
        outer_block = outer_block.title(URL).title_alignment(Alignment::Center);
    }
    f.render_widget(&outer_block, area);
    let [logs_area, ipdb_area, proxies_area, hotkeys_area] =
        Layout::vertical([
            Constraint::Fill(1),
            Constraint::Length(if ipdb_shown { GAUGE_HEIGHT } else { 0 }),
            Constraint::Length(if columns == 0 {
                0
            } else if stacked {
                columns.saturating_add(BORDERS)
            } else {
                COLUMNS_HEIGHT
            }),
            Constraint::Length(HOTKEYS_HEIGHT),
        ])
        .areas(outer_block.inner(area));
    drop(outer_block);

    f.render_widget(
        TuiLoggerWidget::default()
            .block(Block::bordered().title("Logs"))
            .style_error(Style::new().fg(Color::Red))
            .style_warn(Style::new().fg(Color::Yellow))
            .style_info(Style::new().fg(Color::Cyan))
            .style_trace(Style::new().fg(Color::Magenta))
            .style_debug(Style::new().fg(Color::Green))
            .output_target(false)
            .output_file(false)
            .output_line(false)
            .state(logger_state),
        logs_area,
    );

    if ipdb_shown {
        let [asn_area, geo_area] =
            Layout::horizontal([Constraint::Fill(1); 2]).areas(ipdb_area);
        for (gauge_area, title, enabled, progress) in [
            (
                asn_area,
                "ASN database download",
                config.asn_enabled(),
                state.asn_db,
            ),
            (
                geo_area,
                "Geolocation database download",
                config.geolocation_enabled(),
                state.geo_db,
            ),
        ] {
            let (ratio, label) = db_progress(enabled, progress);
            f.render_widget(
                Gauge::default()
                    .block(Block::bordered().title(title))
                    .ratio(ratio)
                    .label(label),
                gauge_area,
            );
        }
    }

    if stacked {
        render_stacked(f, proxies_area, config, state);
    } else {
        render_columns(f, proxies_area, config, state);
    }

    f.render_widget(hotkeys(state), hotkeys_area);
}

fn db_progress(
    enabled: bool,
    progress: DbProgress,
) -> (f64, compact_str::CompactString) {
    const BYTES_PER_MB: f64 = 1_048_576.0_f64;

    if !enabled {
        return (0.0_f64, compact_str::CompactString::const_new("skipped"));
    }
    match progress.finished_ok {
        Some(true) => {
            return (1.0_f64, compact_str::CompactString::const_new("ready"));
        }
        Some(false) => {
            return (0.0_f64, compact_str::CompactString::const_new("failed"));
        }
        None => {}
    }

    let downloaded = (progress.downloaded as f64) / BYTES_PER_MB;
    if progress.total == 0 {
        if progress.downloaded == 0 {
            return (0.0_f64, compact_str::CompactString::const_new("waiting"));
        }
        return (0.0_f64, compact_str::format_compact!("{downloaded:.1} MB"));
    }
    (
        ratio(progress.downloaded, progress.total),
        compact_str::format_compact!(
            "{downloaded:.1}/{:.1} MB",
            (progress.total as f64) / BYTES_PER_MB
        ),
    )
}

fn render_columns(
    f: &mut Frame<'_>,
    area: Rect,
    config: &Config,
    state: &AppState,
) {
    let checking = config.checking.check_url.is_some();
    let layout = Layout::horizontal(std::iter::repeat_n(
        Constraint::Fill(1),
        config.scraping.enabled_protocols.len(),
    ))
    .split(area);

    for (proxy_type, column) in
        config.scraping.enabled_protocols.iter().zip(layout.iter().copied())
    {
        let progress = *state.protocols.get(proxy_type);

        let block = Block::bordered().title(proxy_type.as_str_uppercase());
        f.render_widget(&block, column);

        let [scraping_area, checking_area, working_area] =
            Layout::vertical([Constraint::Fill(1); 3])
                .areas(block.inner(column));
        drop(block);

        f.render_widget(
            Gauge::default()
                .block(Block::bordered().title("Scraping sources"))
                .ratio(ratio(progress.sources_scraped, progress.sources_total))
                .label(compact_str::format_compact!(
                    "{}/{}",
                    progress.sources_scraped,
                    progress.sources_total
                )),
            scraping_area,
        );

        f.render_widget(
            Gauge::default()
                .block(Block::bordered().title("Checking proxies"))
                .ratio(ratio(progress.proxies_checked, progress.proxies_total))
                .label(if checking {
                    compact_str::format_compact!(
                        "{}/{}",
                        progress.proxies_checked,
                        progress.proxies_total
                    )
                } else {
                    compact_str::CompactString::const_new("disabled")
                }),
            checking_area,
        );

        let working_proxies_block = Block::bordered().title("Working proxies");
        f.render_widget(&working_proxies_block, working_area);

        let working = if checking {
            compact_str::format_compact!(
                "{} ({:.1}%)",
                progress.proxies_working,
                if progress.proxies_checked == 0 {
                    0.0_f64
                } else {
                    (progress.proxies_working as f64)
                        / (progress.proxies_checked as f64)
                        * 100.0_f64
                }
            )
        } else {
            compact_str::CompactString::const_new("unknown")
        };
        f.render_widget(
            Line::from(working.as_str()).centered(),
            working_proxies_block.inner(working_area),
        );
    }
}

fn render_stacked(
    f: &mut Frame<'_>,
    area: Rect,
    config: &Config,
    state: &AppState,
) {
    let block = Block::bordered().title("Proxies");
    f.render_widget(&block, area);
    let rows = Layout::vertical(std::iter::repeat_n(
        Constraint::Length(1),
        config.scraping.enabled_protocols.len(),
    ))
    .split(block.inner(area));
    drop(block);

    let checking = config.checking.check_url.is_some();
    for (proxy_type, row) in
        config.scraping.enabled_protocols.iter().zip(rows.iter().copied())
    {
        let name = proxy_type.as_str_uppercase();
        let progress = *state.protocols.get(proxy_type);
        let (done, total, text) = if checking && progress.proxies_checked > 0 {
            (
                progress.proxies_checked,
                progress.proxies_total,
                compact_str::format_compact!(
                    "{name} {}/{} \u{2713}{}",
                    progress.proxies_checked,
                    progress.proxies_total,
                    progress.proxies_working
                ),
            )
        } else {
            (
                progress.sources_scraped,
                progress.sources_total,
                compact_str::format_compact!(
                    "{name} src {}/{}",
                    progress.sources_scraped,
                    progress.sources_total
                ),
            )
        };
        f.render_widget(
            Gauge::default().ratio(ratio(done, total)).label(text),
            row,
        );
    }
}

fn hotkeys(state: &AppState) -> Text<'static> {
    let running = matches!(state.mode, AppMode::Running);
    let mut lines = Vec::with_capacity(usize::from(HOTKEYS_HEIGHT));
    lines.push(Line::raw("Up / PageUp / k - scroll logs up"));
    lines.push(Line::raw("Down / PageDown / j - scroll logs down"));
    if running {
        lines.push(
            Line::raw("ESC / q - stop").style(Style::new().fg(Color::Yellow)),
        );
    }
    lines.push(
        Line::raw(if running {
            "Ctrl-C - quit"
        } else {
            "ESC / q / Ctrl-C - quit"
        })
        .style(Style::new().fg(Color::Red)),
    );

    Text::from(lines).centered()
}

fn handle_key(
    key_event: &crossterm::event::KeyEvent,
    state: &mut AppState,
    token: &tokio_util::sync::CancellationToken,
    logger_state: &TuiWidgetState,
) -> bool {
    let ctrl = key_event.modifiers.contains(KeyModifiers::CONTROL);
    match key_event.code {
        KeyCode::Char('c' | 'C') if ctrl => {
            state.mode = AppMode::Quit;
            token.cancel();
            true
        }
        KeyCode::Esc | KeyCode::Char('q' | 'Q') if !ctrl => {
            state.mode = state.mode.next();
            token.cancel();
            true
        }
        KeyCode::Up | KeyCode::PageUp | KeyCode::Char('k') => {
            logger_state.transition(TuiWidgetEvent::PrevPageKey);
            true
        }
        KeyCode::Down | KeyCode::PageDown | KeyCode::Char('j') => {
            logger_state.transition(TuiWidgetEvent::NextPageKey);
            true
        }
        _ => false,
    }
}

fn handle_app_event(app_event: AppEvent, state: &mut AppState) -> bool {
    let mut redraw_now = false;
    match app_event {
        AppEvent::IpDbTotal(db_type, bytes) => {
            state.db_mut(db_type).total =
                usize::try_from(bytes.unwrap_or_default())
                    .unwrap_or(usize::MAX);
        }
        AppEvent::IpDbDownloaded(db_type, bytes) => {
            let db = state.db_mut(db_type);
            db.downloaded = db.downloaded.saturating_add(bytes);
        }
        AppEvent::IpDbFinished(db_type, ok) => {
            state.db_mut(db_type).finished_ok = Some(ok);
        }
        AppEvent::SourcesTotal(proxy_type, amount) => {
            state.protocols.get_mut(proxy_type).sources_total = amount;
        }
        AppEvent::SourceScraped(proxy_type) => {
            let scraped =
                &mut state.protocols.get_mut(proxy_type).sources_scraped;
            *scraped = scraped.saturating_add(1);
        }
        AppEvent::TotalProxies(proxy_type, amount) => {
            let total = &mut state.protocols.get_mut(proxy_type).proxies_total;
            *total = (*total).max(amount);
        }
        AppEvent::ProxyChecked(proxy_type) => {
            let checked =
                &mut state.protocols.get_mut(proxy_type).proxies_checked;
            *checked = checked.saturating_add(1);
        }
        AppEvent::ProxyWorking(proxy_type) => {
            let working =
                &mut state.protocols.get_mut(proxy_type).proxies_working;
            *working = working.saturating_add(1);
        }
        AppEvent::InputLost => {
            state.input_lost = true;
            if matches!(state.mode, AppMode::Done) {
                state.mode = AppMode::Quit;
            }
            redraw_now = true;
        }
        AppEvent::Done => {
            if matches!(state.mode, AppMode::Running) {
                state.mode = if state.input_lost {
                    AppMode::Quit
                } else {
                    AppMode::Done
                };
            }
            redraw_now = true;
        }
        AppEvent::Quit => {
            state.mode = AppMode::Quit;
            redraw_now = true;
        }
    }
    redraw_now
}

fn handle_event(
    event: Event,
    state: &mut AppState,
    token: &tokio_util::sync::CancellationToken,
    logger_state: &TuiWidgetState,
) -> bool {
    match event {
        Event::Tick => true,
        Event::Crossterm(crossterm_event) => match crossterm_event {
            CrosstermEvent::Key(key_event) if key_event.is_press() => {
                handle_key(&key_event, state, token, logger_state)
            }
            CrosstermEvent::Resize(..) => true,
            _ => false,
        },
        Event::App(app_event) => handle_app_event(app_event, state),
    }
}

#[cfg(test)]
mod tests {
    use crossterm::event::{
        Event as CrosstermEvent, KeyCode, KeyEvent, KeyEventKind, KeyModifiers,
    };
    use tokio_util::sync::CancellationToken;
    use tui_logger::TuiWidgetState;

    use super::{
        AppMode, AppState, DbProgress, Event, db_progress, handle_event, ratio,
    };
    use crate::{event::AppEvent, proxy::ProxyType};

    const MB: usize = 1024 * 1024;

    fn press(code: KeyCode, modifiers: KeyModifiers) -> Event {
        Event::Crossterm(CrosstermEvent::Key(KeyEvent::new(code, modifiers)))
    }

    fn apply(state: &mut AppState, token: &CancellationToken, event: Event) {
        handle_event(event, state, token, &TuiWidgetState::default());
    }

    #[test]
    fn q_stops_the_run_and_then_quits() {
        let mut state = AppState::default();
        let token = CancellationToken::new();
        let q = || press(KeyCode::Char('q'), KeyModifiers::NONE);

        apply(&mut state, &token, q());
        assert!(matches!(state.mode, AppMode::Done));
        assert!(token.is_cancelled());

        apply(&mut state, &token, q());
        assert!(matches!(state.mode, AppMode::Quit));
    }

    #[test]
    fn ctrl_c_quits_at_once_but_ctrl_q_does_nothing() {
        let mut state = AppState::default();
        let token = CancellationToken::new();

        apply(
            &mut state,
            &token,
            press(KeyCode::Char('q'), KeyModifiers::CONTROL),
        );
        assert!(matches!(state.mode, AppMode::Running));
        assert!(!token.is_cancelled());

        apply(
            &mut state,
            &token,
            press(KeyCode::Char('c'), KeyModifiers::CONTROL),
        );
        assert!(matches!(state.mode, AppMode::Quit));
        assert!(token.is_cancelled());
    }

    #[test]
    fn key_release_is_ignored() {
        let mut state = AppState::default();
        let token = CancellationToken::new();
        let mut release = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
        release.kind = KeyEventKind::Release;

        apply(
            &mut state,
            &token,
            Event::Crossterm(CrosstermEvent::Key(release)),
        );
        assert!(matches!(state.mode, AppMode::Running));
    }

    #[test]
    fn finishing_never_undoes_a_quit() {
        let mut state = AppState::default();
        let token = CancellationToken::new();

        apply(&mut state, &token, Event::App(AppEvent::Quit));
        apply(&mut state, &token, Event::App(AppEvent::Done));
        assert!(matches!(state.mode, AppMode::Quit));
    }

    #[test]
    fn finishing_waits_for_a_key_only_if_keys_still_work() {
        let token = CancellationToken::new();

        let mut state = AppState::default();
        apply(&mut state, &token, Event::App(AppEvent::Done));
        assert!(matches!(state.mode, AppMode::Done));
        apply(&mut state, &token, Event::App(AppEvent::InputLost));
        assert!(matches!(state.mode, AppMode::Quit));

        let mut state = AppState::default();
        apply(&mut state, &token, Event::App(AppEvent::InputLost));
        assert!(matches!(state.mode, AppMode::Running));
        apply(&mut state, &token, Event::App(AppEvent::Done));
        assert!(matches!(state.mode, AppMode::Quit));
    }

    #[test]
    fn proxy_total_only_grows() {
        let mut state = AppState::default();
        let token = CancellationToken::new();
        let total = |amount| {
            Event::App(AppEvent::TotalProxies(ProxyType::Http, amount))
        };

        apply(&mut state, &token, total(5));
        apply(&mut state, &token, total(3));
        assert_eq!(state.protocols.get(ProxyType::Http).proxies_total, 5);
    }

    #[test]
    fn ratio_stays_in_the_range_gauge_accepts() {
        assert!(ratio(1, 0).abs() < f64::EPSILON);
        assert!((ratio(11, 10) - 1.0_f64).abs() < f64::EPSILON);
        assert!((ratio(1, 4) - 0.25_f64).abs() < f64::EPSILON);
    }

    #[test]
    fn db_progress_labels() {
        let label = |enabled, progress| db_progress(enabled, progress).1;
        let progress = |finished_ok, downloaded, total| DbProgress {
            finished_ok,
            downloaded,
            total,
        };

        assert_eq!(label(false, progress(Some(true), 0, 0)), "skipped");
        assert_eq!(label(true, progress(Some(true), 0, 0)), "ready");
        assert_eq!(label(true, progress(Some(false), MB, MB)), "failed");
        assert_eq!(label(true, progress(None, 0, 0)), "waiting");
        assert_eq!(label(true, progress(None, MB, 0)), "1.0 MB");
        assert_eq!(label(true, progress(None, MB, 2 * MB)), "1.0/2.0 MB");
    }
}
