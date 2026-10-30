#![expect(clippy::wildcard_enum_match_arm)]

use std::{sync::Arc, time::Duration};

use crossterm::event::{Event as CrosstermEvent, KeyCode, KeyModifiers};
use futures::StreamExt as _;
use tui_logger::{TuiWidgetEvent, TuiWidgetState};

use super::view;
use crate::{
    config::Config,
    event::{AppEvent, Event, Stage},
    ipdb,
    proxy::PerProxyType,
};

const TICK: Duration = Duration::from_millis(100);
const TICKS_PER_SECOND: u64 = 10;
// A speed measured over less than this swings too much to be worth showing.
const MIN_TICKS_FOR_SPEED: u64 = 3 * TICKS_PER_SECOND;
const MAX_INPUT_ERRORS: u8 = 8;

pub struct RatatuiRestoreGuard;

impl Drop for RatatuiRestoreGuard {
    fn drop(&mut self) {
        ratatui::restore();
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
            sources_total: self
                .sources_total
                .saturating_add(other.sources_total),
            sources_scraped: self
                .sources_scraped
                .saturating_add(other.sources_scraped),
            proxies_total: self
                .proxies_total
                .saturating_add(other.proxies_total),
            proxies_checked: self
                .proxies_checked
                .saturating_add(other.proxies_checked),
            proxies_working: self
                .proxies_working
                .saturating_add(other.proxies_working),
        }
    }
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
    pub found_this_second: u64,
    pub found_per_second: Vec<u64>,
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

    fn tick(&mut self) {
        if matches!(self.mode, Mode::Done | Mode::Quit) {
            return;
        }
        self.ticks = self.ticks.saturating_add(1);
        if self.stage == Stage::Checking
            && self.ticks.is_multiple_of(TICKS_PER_SECOND)
        {
            self.found_per_second
                .push(std::mem::take(&mut self.found_this_second));
        }
    }

    pub const fn elapsed_seconds(&self) -> u64 {
        self.ticks.div_euclid(TICKS_PER_SECOND)
    }

    // Averaged over all of checking: each check takes about as long as any
    // other, and a short window would only make the time left jump around.
    pub fn checks_per_second(&self) -> Option<usize> {
        let ticks = self.ticks.saturating_sub(self.checking_since);
        if self.stage != Stage::Checking || ticks < MIN_TICKS_FOR_SPEED {
            return None;
        }
        let speed = (self.proxies_checked as u64)
            .saturating_mul(TICKS_PER_SECOND)
            .checked_div(ticks)?;
        usize::try_from(speed).ok().filter(|&speed| speed != 0)
    }

    pub fn seconds_left(&self, config: &Config) -> Option<u64> {
        let speed = self.checks_per_second()?;
        let total: usize = config
            .scraping
            .enabled_protocols
            .iter()
            .map(|protocol| self.protocols.get(protocol).proxies_total)
            .sum();
        let left = total.saturating_sub(self.proxies_checked).div_ceil(speed);
        Some(left as u64)
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

    let mut app_state = State::default();
    let logger_state = TuiWidgetState::default();

    terminal
        .draw(|frame| view::draw(frame, &config, &app_state, &logger_state))?;

    while !matches!(app_state.mode, Mode::Quit) {
        let Some(event) = rx.recv().await else {
            break;
        };
        let mut redraw =
            handle_event(event, &mut app_state, &token, &logger_state);

        while !matches!(app_state.mode, Mode::Quit)
            && let Ok(event) = rx.try_recv()
        {
            redraw |=
                handle_event(event, &mut app_state, &token, &logger_state);
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
            state.stopped |= matches!(state.mode, Mode::Running);
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
            state.proxies_checked = state.proxies_checked.saturating_add(1);
        }
        AppEvent::ProxyWorking(proxy_type) => {
            let working =
                &mut state.protocols.get_mut(proxy_type).proxies_working;
            *working = working.saturating_add(1);
            state.found_this_second = state.found_this_second.saturating_add(1);
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
                state.mode =
                    if state.input_lost { Mode::Quit } else { Mode::Done };
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
    use crossterm::event::{
        Event as CrosstermEvent, KeyCode, KeyEvent, KeyEventKind, KeyModifiers,
    };
    use tokio_util::sync::CancellationToken;
    use tui_logger::TuiWidgetState;

    use super::{Event, Mode, State, TICKS_PER_SECOND, handle_event};
    use crate::{
        config::tests::test_config,
        event::{AppEvent, Stage},
        proxy::ProxyType,
    };

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
    fn ctrl_c_quits_at_once_but_ctrl_q_does_nothing() {
        let mut state = State::default();
        let token = CancellationToken::new();

        apply(
            &mut state,
            &token,
            press(KeyCode::Char('q'), KeyModifiers::CONTROL),
        );
        assert!(matches!(state.mode, Mode::Running));
        assert!(!token.is_cancelled());

        apply(
            &mut state,
            &token,
            press(KeyCode::Char('c'), KeyModifiers::CONTROL),
        );
        assert!(matches!(state.mode, Mode::Quit));
        assert!(token.is_cancelled());
    }

    #[test]
    fn key_release_is_ignored() {
        let mut state = State::default();
        let token = CancellationToken::new();
        let mut release = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
        release.kind = KeyEventKind::Release;

        apply(
            &mut state,
            &token,
            Event::Crossterm(CrosstermEvent::Key(release)),
        );
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
        let total = |amount| {
            Event::App(AppEvent::TotalProxies(ProxyType::Http, amount))
        };

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
    fn working_proxies_are_counted_per_second_of_checking() {
        let mut state = State::default();
        let token = CancellationToken::new();
        let seconds = |state: &mut State, amount: u64| {
            for _ in 0..amount * TICKS_PER_SECOND {
                apply(state, &token, Event::Tick);
            }
        };

        seconds(&mut state, 2);
        assert_eq!(state.found_per_second, [0_u64; 0]);

        apply(&mut state, &token, Event::App(AppEvent::Stage(Stage::Checking)));
        for _ in 0_u8..3 {
            apply(
                &mut state,
                &token,
                Event::App(AppEvent::ProxyWorking(ProxyType::Http)),
            );
        }
        seconds(&mut state, 2);
        assert_eq!(state.found_per_second, [3, 0]);
    }

    #[tokio::test]
    async fn time_left_comes_from_the_speed_since_checking_began() {
        let config = test_config(|_| {}).await;
        let mut state = State {
            stage: Stage::Checking,
            ticks: 100,
            checking_since: 100,
            proxies_checked: 500,
            ..State::default()
        };
        state.protocols.get_mut(ProxyType::Http).proxies_total = 2_000;

        assert_eq!(state.seconds_left(&config), None);

        state.ticks = 150;
        assert_eq!(state.checks_per_second(), Some(100));
        assert_eq!(state.seconds_left(&config), Some(15));

        state.stage = Stage::Saving;
        assert_eq!(state.seconds_left(&config), None);
    }
}
