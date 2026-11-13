#![expect(clippy::wildcard_enum_match_arm)]

use std::{mem::ManuallyDrop, sync::Arc, time::Duration};

use compact_str::CompactString;
use crossterm::event::{Event as CrosstermEvent, KeyCode, KeyModifiers};
use futures::StreamExt as _;
use tui_logger::{TuiWidgetEvent, TuiWidgetState};

use super::view;
use crate::{
    config::Config,
    event::{AppEvent, Event, Stage},
    ipdb,
    proxy::{PerProxyType, ProxyType},
};

// Keys, resizes and stage changes redraw at once, so this only paces the
// counters. Going faster spins their last digits past reading and has the
// terminal repaint for nothing, which is what SSH and phones feel.
pub const TICKS_PER_SECOND: u64 = 4;
const TICK: Duration = Duration::from_millis(1000_u64.div_euclid(TICKS_PER_SECOND));
// Until the first checks time out, only the quick ones have finished: with the
// default 10 s timeout, the time left starts out about five times too long.
const MIN_TICKS_FOR_SPEED: u64 = 15 * TICKS_PER_SECOND;
const MAX_INPUT_ERRORS: u8 = 8;
pub const FASTEST_KEPT: usize = 8;

pub struct RatatuiRestoreGuard;

impl Drop for RatatuiRestoreGuard {
    // Not ratatui::restore, which panics if stderr went with the terminal. The
    // panic hook has already restored it, and leaving the alternate screen
    // twice would put the cursor back over the panic report.
    fn drop(&mut self) {
        if crossterm::terminal::is_raw_mode_enabled().unwrap_or(true) {
            drop(ratatui::try_restore());
        }
        drop(crossterm::execute!(std::io::stdout(), crossterm::cursor::Show));
    }
}

#[derive(Default)]
pub enum Mode {
    #[default]
    Running,
    Stopping,
    Done,
    Quit,
}

impl Mode {
    const fn next(&self) -> Self {
        match self {
            Self::Running => Self::Stopping,
            Self::Stopping | Self::Done | Self::Quit => Self::Quit,
        }
    }
}

#[derive(Clone, Copy, Default)]
pub struct DbProgress {
    pub finished_ok: Option<bool>,
    pub downloaded: usize,
    pub total: usize,
}

#[derive(Clone, Copy, Default)]
pub struct ProtocolProgress {
    pub sources_total: usize,
    pub sources_scraped: usize,
    pub proxies_total: usize,
    pub proxies_checked: usize,
    pub proxies_working: usize,
}

impl ProtocolProgress {
    pub const fn plus(self, other: Self) -> Self {
        Self {
            sources_total: self.sources_total.saturating_add(other.sources_total),
            sources_scraped: self.sources_scraped.saturating_add(other.sources_scraped),
            proxies_total: self.proxies_total.saturating_add(other.proxies_total),
            proxies_checked: self.proxies_checked.saturating_add(other.proxies_checked),
            proxies_working: self.proxies_working.saturating_add(other.proxies_working),
        }
    }
}

#[derive(Clone)]
pub struct FastProxy {
    pub protocol: ProxyType,
    pub address: CompactString,
    pub latency: Duration,
}

#[derive(Default)]
pub struct State {
    pub mode: Mode,
    pub stage: Stage,
    pub stopped: bool,
    pub input_lost: bool,
    pub saved: Option<usize>,
    pub ticks: u64,
    pub checking_since: u64,
    pub proxies_checked: usize,
    pub fastest: Vec<FastProxy>,
    pub asn_db: DbProgress,
    pub geo_db: DbProgress,
    pub protocols: PerProxyType<ProtocolProgress>,
}

impl State {
    const fn db_mut(&mut self, db_type: ipdb::DbType) -> &mut DbProgress {
        match db_type {
            ipdb::DbType::Asn => &mut self.asn_db,
            ipdb::DbType::Geo => &mut self.geo_db,
        }
    }

    const fn tick(&mut self) {
        if matches!(self.mode, Mode::Done | Mode::Quit) {
            return;
        }
        self.ticks = self.ticks.saturating_add(1);
    }

    fn rank(&mut self, proxy: FastProxy) {
        let place = self.fastest.partition_point(move |other| other.latency <= proxy.latency);
        if place < FASTEST_KEPT {
            self.fastest.insert(place, proxy);
            self.fastest.truncate(FASTEST_KEPT);
        }
    }

    pub const fn elapsed_seconds(&self) -> u64 {
        self.ticks.div_euclid(TICKS_PER_SECOND)
    }

    // Averaged over all of checking: each check takes about as long as any
    // other, and a short window would only make the time left jump around.
    fn checking_ticks(&self) -> Option<u64> {
        let ticks = self.ticks.saturating_sub(self.checking_since);
        (self.stage == Stage::Checking && ticks >= MIN_TICKS_FOR_SPEED).then_some(ticks)
    }

    pub fn checks_per_second(&self) -> Option<usize> {
        let speed = (self.proxies_checked as u64)
            .saturating_mul(TICKS_PER_SECOND)
            .checked_div(self.checking_ticks()?)?;
        usize::try_from(speed).ok().filter(|&speed| speed != 0)
    }

    // Not from the rounded speed: at 1.9 checks/s that alone would nearly
    // double the estimate.
    pub fn seconds_left(&self, config: &Config) -> Option<u64> {
        let ticks = self.checking_ticks()?;
        let total: usize = config
            .scraping
            .enabled_protocols
            .iter()
            .map(move |protocol| self.protocols.get(protocol).proxies_total)
            .sum();
        let left = total.saturating_sub(self.proxies_checked) as u64;
        let checked_ticks = (self.proxies_checked as u64).saturating_mul(TICKS_PER_SECOND);
        (checked_ticks != 0).then(move || left.saturating_mul(ticks).div_ceil(checked_ticks))
    }
}

pub async fn run(
    terminal: ratatui::DefaultTerminal,
    config: Arc<Config>,
    token: tokio_util::sync::CancellationToken,
    tx: tokio::sync::mpsc::UnboundedSender<Event>,
    mut rx: tokio::sync::mpsc::UnboundedReceiver<Event>,
) -> crate::Result<()> {
    // Dropping it panics if stderr went with the terminal, and it only shows
    // the cursor, which RatatuiRestoreGuard does.
    let mut terminal = ManuallyDrop::new(terminal);

    tokio::spawn(tick_event_listener(tx.clone()));
    tokio::spawn(crossterm_event_listener(tx));

    let mut app_state = State::default();
    let logger_state = TuiWidgetState::default();

    terminal.draw(|frame| view::draw(frame, &config, &app_state, &logger_state))?;

    while !matches!(app_state.mode, Mode::Quit) {
        let Some(event) = rx.recv().await else {
            break;
        };
        let mut redraw = handle_event(event, &mut app_state, &token, &logger_state);

        while !matches!(app_state.mode, Mode::Quit)
            && let Ok(event) = rx.try_recv()
        {
            redraw |= handle_event(event, &mut app_state, &token, &logger_state);
        }

        if redraw {
            terminal.draw(|frame| {
                view::draw(frame, &config, &app_state, &logger_state);
            })?;
        }
    }
    Ok(())
}

async fn tick_event_listener(tx: tokio::sync::mpsc::UnboundedSender<Event>) {
    let mut tick = tokio::time::interval(TICK);
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

async fn crossterm_event_listener(tx: tokio::sync::mpsc::UnboundedSender<Event>) {
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
    drop(reader);

    tracing::warn!(
        "Terminal input was lost, so the hotkeys no longer work. The program will exit when the \
         run finishes."
    );
    drop(tx.send(Event::App(AppEvent::InputLost)));
}

fn handle_key(
    key_event: &crossterm::event::KeyEvent,
    state: &mut State,
    token: &tokio_util::sync::CancellationToken,
    logger_state: &TuiWidgetState,
) -> bool {
    let ctrl = key_event.modifiers.contains(KeyModifiers::CONTROL);
    match key_event.code {
        KeyCode::Char('c' | 'C') if ctrl => {
            state.mode = Mode::Quit;
            token.cancel();
            true
        }
        KeyCode::Esc | KeyCode::Char('q' | 'Q') if !ctrl => {
            // Everything is checked by the time saving starts.
            state.stopped |= matches!(state.mode, Mode::Running) && state.stage != Stage::Saving;
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
        KeyCode::End => {
            logger_state.transition(TuiWidgetEvent::EscapeKey);
            true
        }
        _ => false,
    }
}

fn handle_app_event(app_event: AppEvent, state: &mut State) -> bool {
    let mut redraw_now = false;
    match app_event {
        AppEvent::Stage(next) => {
            state.stage = next;
            if next == Stage::Checking {
                state.checking_since = state.ticks;
            }
            redraw_now = true;
        }
        AppEvent::IpDbTotal(db_type, bytes) => {
            state.db_mut(db_type).total =
                usize::try_from(bytes.unwrap_or_default()).unwrap_or(usize::MAX);
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
            let scraped = &mut state.protocols.get_mut(proxy_type).sources_scraped;
            *scraped = scraped.saturating_add(1);
        }
        AppEvent::TotalProxies(proxy_type, amount) => {
            let total = &mut state.protocols.get_mut(proxy_type).proxies_total;
            *total = (*total).max(amount);
        }
        AppEvent::ProxyChecked(proxy_type) => {
            let checked = &mut state.protocols.get_mut(proxy_type).proxies_checked;
            *checked = checked.saturating_add(1);
            state.proxies_checked = state.proxies_checked.saturating_add(1);
        }
        AppEvent::ProxyWorking { protocol, address, latency } => {
            let working = &mut state.protocols.get_mut(protocol).proxies_working;
            *working = working.saturating_add(1);
            state.rank(FastProxy { protocol, address, latency });
        }
        AppEvent::Saved(amount) => {
            state.saved = Some(amount);
        }
        AppEvent::InputLost => {
            state.input_lost = true;
            if matches!(state.mode, Mode::Done) {
                state.mode = Mode::Quit;
            }
            redraw_now = true;
        }
        AppEvent::Done => {
            if matches!(state.mode, Mode::Running | Mode::Stopping) {
                state.mode = if state.input_lost { Mode::Quit } else { Mode::Done };
            }
            redraw_now = true;
        }
        AppEvent::Quit => {
            state.mode = Mode::Quit;
            redraw_now = true;
        }
    }
    redraw_now
}

fn handle_event(
    event: Event,
    state: &mut State,
    token: &tokio_util::sync::CancellationToken,
    logger_state: &TuiWidgetState,
) -> bool {
    match event {
        // Still redraws once the run is over: the logger hands over its last
        // lines from another thread, after the event that ended the run.
        Event::Tick => {
            state.tick();
            true
        }
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
    use std::time::Duration;

    use crossterm::event::{
        Event as CrosstermEvent, KeyCode, KeyEvent, KeyEventKind, KeyModifiers,
    };
    use tokio_util::sync::CancellationToken;
    use tui_logger::TuiWidgetState;

    use super::{
        Event, FASTEST_KEPT, Mode, State, TICK, TICKS_PER_SECOND, handle_event, tick_event_listener,
    };
    use crate::{
        config::tests::test_config,
        event::{AppEvent, Stage},
        ipdb::DbType,
        proxy::ProxyType,
    };

    #[test]
    fn a_proxy_slower_than_all_the_fastest_is_not_kept() {
        let mut state = State::default();
        let token = CancellationToken::new();
        let working = |millis| {
            Event::App(AppEvent::ProxyWorking {
                protocol: ProxyType::Http,
                address: "1.1.1.1:80".into(),
                latency: Duration::from_millis(millis),
            })
        };

        for millis in 1..=FASTEST_KEPT as u64 {
            apply(&mut state, &token, working(millis));
        }
        apply(&mut state, &token, working(1_000));

        assert_eq!(state.fastest.len(), FASTEST_KEPT);
        let slowest = state.fastest.last().map(|proxy| proxy.latency);
        assert_eq!(slowest, Some(Duration::from_millis(FASTEST_KEPT as u64)));
    }

    #[test]
    fn the_progress_of_downloads_and_protocols_adds_up() {
        let mut state = State { ticks: 3, ..State::default() };
        let token = CancellationToken::new();
        for event in [
            AppEvent::IpDbTotal(DbType::Asn, Some(2_000_000)),
            AppEvent::IpDbDownloaded(DbType::Asn, 500),
            AppEvent::IpDbDownloaded(DbType::Asn, 500),
            AppEvent::IpDbTotal(DbType::Geo, None),
            AppEvent::IpDbFinished(DbType::Geo, false),
            AppEvent::SourcesTotal(ProxyType::Socks4, 5),
            AppEvent::SourceScraped(ProxyType::Socks4),
            AppEvent::SourceScraped(ProxyType::Socks4),
            AppEvent::Stage(Stage::Checking),
            AppEvent::ProxyChecked(ProxyType::Socks4),
            AppEvent::ProxyChecked(ProxyType::Socks5),
            AppEvent::Saved(7),
        ] {
            apply(&mut state, &token, Event::App(event));
        }

        assert_eq!((state.asn_db.downloaded, state.asn_db.total), (1_000, 2_000_000));
        assert_eq!(state.asn_db.finished_ok, None);
        assert_eq!((state.geo_db.total, state.geo_db.finished_ok), (0, Some(false)));
        let socks4 = state.protocols.get(ProxyType::Socks4);
        assert_eq!((socks4.sources_total, socks4.sources_scraped), (5, 2));
        assert_eq!(socks4.proxies_checked, 1);
        assert_eq!(state.proxies_checked, 2);
        assert_eq!(state.checking_since, 3);
        assert!(state.stage == Stage::Checking);
        assert_eq!(state.saved, Some(7));
    }

    #[test]
    fn only_what_changes_the_screen_redraws_it() {
        let mut state = State::default();
        let token = CancellationToken::new();
        let logger_state = TuiWidgetState::default();
        let mut redraws = |event| handle_event(event, &mut state, &token, &logger_state);

        for code in [
            KeyCode::Up,
            KeyCode::PageUp,
            KeyCode::Char('k'),
            KeyCode::Down,
            KeyCode::PageDown,
            KeyCode::Char('j'),
            KeyCode::End,
        ] {
            assert!(redraws(press(code, KeyModifiers::NONE)));
        }
        assert!(redraws(Event::Crossterm(CrosstermEvent::Resize(80, 24))));
        assert!(!redraws(press(KeyCode::Char('x'), KeyModifiers::NONE)));
        assert!(!redraws(Event::Crossterm(CrosstermEvent::FocusGained)));
        assert!(!redraws(Event::App(AppEvent::Saved(1))));
        assert!(redraws(Event::App(AppEvent::Stage(Stage::Saving))));
        assert!(matches!(state.mode, Mode::Running));
        assert!(!token.is_cancelled());
    }

    fn press(code: KeyCode, modifiers: KeyModifiers) -> Event {
        Event::Crossterm(CrosstermEvent::Key(KeyEvent::new(code, modifiers)))
    }

    fn apply(state: &mut State, token: &CancellationToken, event: Event) {
        handle_event(event, state, token, &TuiWidgetState::default());
    }

    #[test]
    fn q_stops_the_run_and_then_quits() {
        let mut state = State::default();
        let token = CancellationToken::new();
        let q = || press(KeyCode::Char('q'), KeyModifiers::NONE);

        apply(&mut state, &token, q());
        assert!(matches!(state.mode, Mode::Stopping));
        assert!(state.stopped);
        assert!(token.is_cancelled());

        apply(&mut state, &token, Event::App(AppEvent::Done));
        assert!(matches!(state.mode, Mode::Done));

        apply(&mut state, &token, q());
        assert!(matches!(state.mode, Mode::Quit));
    }

    #[test]
    fn q_while_saving_is_not_a_stopped_run() {
        let mut state = State { stage: Stage::Saving, ..State::default() };
        let token = CancellationToken::new();

        apply(&mut state, &token, press(KeyCode::Char('q'), KeyModifiers::NONE));

        assert!(matches!(state.mode, Mode::Stopping));
        assert!(!state.stopped);
    }

    #[test]
    fn ctrl_c_quits_at_once_but_ctrl_q_does_nothing() {
        let mut state = State::default();
        let token = CancellationToken::new();

        apply(&mut state, &token, press(KeyCode::Char('q'), KeyModifiers::CONTROL));
        assert!(matches!(state.mode, Mode::Running));
        assert!(!token.is_cancelled());

        apply(&mut state, &token, press(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert!(matches!(state.mode, Mode::Quit));
        assert!(token.is_cancelled());
    }

    #[test]
    fn key_release_is_ignored() {
        let mut state = State::default();
        let token = CancellationToken::new();
        let mut release = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
        release.kind = KeyEventKind::Release;

        apply(&mut state, &token, Event::Crossterm(CrosstermEvent::Key(release)));
        assert!(matches!(state.mode, Mode::Running));
    }

    #[test]
    fn finishing_never_undoes_a_quit() {
        let mut state = State::default();
        let token = CancellationToken::new();

        apply(&mut state, &token, Event::App(AppEvent::Quit));
        apply(&mut state, &token, Event::App(AppEvent::Done));
        assert!(matches!(state.mode, Mode::Quit));
    }

    #[test]
    fn finishing_waits_for_a_key_only_if_keys_still_work() {
        let token = CancellationToken::new();

        let mut state = State::default();
        apply(&mut state, &token, Event::App(AppEvent::Done));
        assert!(matches!(state.mode, Mode::Done));
        apply(&mut state, &token, Event::App(AppEvent::InputLost));
        assert!(matches!(state.mode, Mode::Quit));

        let mut state = State::default();
        apply(&mut state, &token, Event::App(AppEvent::InputLost));
        assert!(matches!(state.mode, Mode::Running));
        apply(&mut state, &token, Event::App(AppEvent::Done));
        assert!(matches!(state.mode, Mode::Quit));
    }

    #[test]
    fn proxy_total_only_grows() {
        let mut state = State::default();
        let token = CancellationToken::new();
        let total = |amount| Event::App(AppEvent::TotalProxies(ProxyType::Http, amount));

        apply(&mut state, &token, total(5));
        apply(&mut state, &token, total(3));
        assert_eq!(state.protocols.get(ProxyType::Http).proxies_total, 5);
    }

    #[test]
    fn the_clock_stops_when_the_run_does() {
        let mut state = State::default();
        let token = CancellationToken::new();

        apply(&mut state, &token, Event::Tick);
        apply(&mut state, &token, Event::App(AppEvent::Done));
        apply(&mut state, &token, Event::Tick);
        assert_eq!(state.ticks, 1);
    }

    #[test]
    fn only_the_fastest_proxies_are_kept_in_order() {
        let mut state = State::default();
        let token = CancellationToken::new();

        for millis in (1..=FASTEST_KEPT as u64 * 2).rev() {
            apply(
                &mut state,
                &token,
                Event::App(AppEvent::ProxyWorking {
                    protocol: ProxyType::Http,
                    address: "1.1.1.1:80".into(),
                    latency: Duration::from_millis(millis),
                }),
            );
        }

        let kept: Vec<_> = state.fastest.iter().map(|proxy| proxy.latency).collect();
        let expected: Vec<_> = (1..=FASTEST_KEPT as u64).map(Duration::from_millis).collect();
        assert_eq!(kept, expected);
        assert_eq!(state.protocols.get(ProxyType::Http).proxies_working, FASTEST_KEPT * 2);
    }

    #[tokio::test]
    async fn time_left_comes_from_the_speed_since_checking_began() {
        let config = test_config(|_| {}).await;
        let mut state = State {
            stage: Stage::Checking,
            ticks: 10 * TICKS_PER_SECOND,
            checking_since: 10 * TICKS_PER_SECOND,
            proxies_checked: 1_500,
            ..State::default()
        };
        state.protocols.get_mut(ProxyType::Http).proxies_total = 2_000;

        assert_eq!(state.seconds_left(&config), None);

        state.ticks = 24 * TICKS_PER_SECOND;
        assert_eq!(state.checks_per_second(), None);

        state.ticks = 25 * TICKS_PER_SECOND;
        assert_eq!(state.checks_per_second(), Some(100));
        assert_eq!(state.seconds_left(&config), Some(5));

        state.stage = Stage::Saving;
        assert_eq!(state.seconds_left(&config), None);
    }

    #[tokio::test]
    async fn time_left_is_not_thrown_off_by_a_low_speed() {
        let config = test_config(|_| {}).await;
        let mut state = State {
            stage: Stage::Checking,
            ticks: 20 * TICKS_PER_SECOND,
            proxies_checked: 38,
            ..State::default()
        };
        state.protocols.get_mut(ProxyType::Http).proxies_total = 100;

        assert_eq!(state.checks_per_second(), Some(1));
        assert_eq!(state.seconds_left(&config), Some(33));

        state.proxies_checked = 5;
        assert_eq!(state.checks_per_second(), None);
        assert_eq!(state.seconds_left(&config), Some(380));

        state.proxies_checked = 0;
        assert_eq!(state.seconds_left(&config), None);
    }

    #[tokio::test(start_paused = true)]
    async fn ticks_keep_their_pace_until_nobody_listens() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let ticks = tokio::spawn(tick_event_listener(tx));
        let started = tokio::time::Instant::now();

        for tick in 0..TICKS_PER_SECOND {
            assert!(matches!(rx.recv().await, Some(Event::Tick)));
            assert_eq!(started.elapsed(), TICK.saturating_mul(u32::try_from(tick).unwrap()));
        }
        drop(rx);

        ticks.await.unwrap();
    }
}
