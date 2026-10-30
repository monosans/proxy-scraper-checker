use compact_str::{CompactString, ToCompactString as _, format_compact};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Margin, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Paragraph, Sparkline},
};
use tui_logger::{ExtLogRecord, LogFormatter, TuiLoggerWidget, TuiWidgetState};

use super::app::{DbProgress, Mode, ProtocolProgress, State};
use crate::{config::Config, event::Stage};

const NAME_WIDTH: usize = 6;
const SHARE_WIDTH: usize = 6;
const COLUMN_GAP: usize = 2;
const MIN_BAR_WIDTH: usize = 10;
const LOG_INDENT: usize = 10;
const LOG_MIN_WIDTH: u16 = 18;
// Below this the blank lines and the column headings go to the logs.
const ROOMY_HEIGHT: u16 = 20;
const CHART_MIN_HEIGHT: u16 = 28;
const CHART_HEIGHT: u16 = 4;
const CHART_BARS_HEIGHT: u16 = 2;
const TITLE: &str = "proxy-scraper-checker";
const URL: &str = "https://github.com/monosans/proxy-scraper-checker";

const FILLED: &str = "\u{2501}";
const FILLED_HALF: &str = "\u{257e}";
const TRACK: &str = "\u{2500}";

const fn dim() -> Style {
    Style::new().add_modifier(Modifier::DIM)
}

const fn bold() -> Style {
    Style::new().add_modifier(Modifier::BOLD)
}

// Green only ever means a working proxy, so it needs no legend.
const fn working() -> Style {
    Style::new().fg(Color::Green)
}

const fn warning() -> Style {
    Style::new().fg(Color::Yellow)
}

const fn error() -> Style {
    Style::new().fg(Color::Red)
}

fn gap() -> Span<'static> {
    Span::raw(" ".repeat(COLUMN_GAP))
}

fn line_width(spans: &[Span<'_>]) -> usize {
    spans.iter().map(Span::width).sum()
}

fn grouped(number: usize) -> CompactString {
    let digits = number.to_compact_string();
    let mut grouped = CompactString::default();
    for (remaining, digit) in (1..=digits.len()).rev().zip(digits.chars()) {
        grouped.push(digit);
        if remaining != 1 && remaining.saturating_sub(1).is_multiple_of(3) {
            grouped.push(',');
        }
    }
    grouped
}

fn clock(seconds: u64) -> CompactString {
    let minutes = seconds.div_euclid(60);
    let hours = minutes.div_euclid(60);
    if hours == 0 {
        format_compact!("{minutes}:{:02}", seconds.rem_euclid(60))
    } else {
        format_compact!(
            "{hours}:{:02}:{:02}",
            minutes.rem_euclid(60),
            seconds.rem_euclid(60)
        )
    }
}

fn megabytes(bytes: usize) -> f64 {
    (bytes as f64) / 1_000_000.0_f64
}

fn scale(part: usize, whole: usize, steps: usize) -> usize {
    let scaled = (part as u128)
        .saturating_mul(steps as u128)
        .checked_div(whole as u128)
        .unwrap_or_default();
    usize::try_from(scaled).unwrap_or(steps).min(steps)
}

#[derive(Clone, Copy, Default)]
struct Meter {
    done: usize,
    total: usize,
    working: usize,
}

impl Meter {
    // What the run has sifted so far: the proxies that passed in green, the
    // ones that failed plainly, and a thinner track for what is left. A half
    // cell shows movement on narrow terminals, and a fill reaches the end only
    // when the work has.
    fn spans(self, width: usize) -> Vec<Span<'static>> {
        let half_cells = scale(self.done, self.total, width.saturating_mul(2));
        let full = half_cells.div_euclid(2);
        let tip = half_cells.rem_euclid(2);
        let passed = scale(self.working, self.total, width)
            .max(usize::from(self.working > 0))
            .min(full);

        let mut spans = vec![
            Span::styled(FILLED.repeat(passed), working()),
            Span::raw(FILLED.repeat(full.saturating_sub(passed))),
        ];
        if tip == 1 {
            spans.push(Span::raw(FILLED_HALF));
        }
        spans.push(Span::styled(
            TRACK.repeat(width.saturating_sub(full).saturating_sub(tip)),
            dim(),
        ));
        spans
    }
}

struct Row {
    name: &'static str,
    meter: Meter,
    count: CompactString,
    count_style: Style,
    amount: CompactString,
    share: CompactString,
}

struct Headings {
    count: &'static str,
    amount: &'static str,
}

#[derive(Default)]
struct Columns {
    bar: usize,
    count: usize,
    amount: usize,
    share: bool,
}

impl Columns {
    // Numbers keep their full width and the bar takes what is left; on narrow
    // terminals the percentage goes first, then the bar.
    fn fit<'a>(
        width: usize,
        rows: impl Iterator<Item = &'a Row>,
        headings: &Headings,
    ) -> Self {
        let mut columns = Self {
            count: headings.count.len(),
            amount: headings.amount.len(),
            ..Self::default()
        };
        let mut any_share = false;
        for row in rows {
            columns.count = columns.count.max(row.count.len());
            columns.amount = columns.amount.max(row.amount.len());
            any_share |= !row.share.is_empty();
        }

        let fixed: usize =
            [NAME_WIDTH, columns.count, columns.amount, 3 * COLUMN_GAP]
                .into_iter()
                .sum();
        let room = width.saturating_sub(fixed);
        let room_with_share =
            room.saturating_sub(SHARE_WIDTH.saturating_add(COLUMN_GAP));

        columns.share = any_share && room_with_share >= MIN_BAR_WIDTH;
        columns.bar = if columns.share { room_with_share } else { room };
        if columns.bar < MIN_BAR_WIDTH {
            columns.bar = 0;
        }
        columns
    }

    fn heading(&self, headings: &Headings) -> Line<'static> {
        let mut spans = vec![Span::raw(" ".repeat(NAME_WIDTH))];
        if self.bar != 0 {
            spans.push(gap());
            spans.push(Span::raw(" ".repeat(self.bar)));
        }
        spans.push(gap());
        spans.push(Span::styled(
            format_compact!("{:>1$}", headings.count, self.count).to_string(),
            dim(),
        ));
        spans.push(gap());
        spans.push(Span::styled(
            format_compact!("{:>1$}", headings.amount, self.amount).to_string(),
            if headings.amount == "Working" { working() } else { dim() },
        ));
        Line::from(spans)
    }

    fn row(&self, row: &Row) -> Line<'static> {
        let mut spans = vec![Span::raw(
            format_compact!("{:<NAME_WIDTH$}", row.name).to_string(),
        )];
        if self.bar != 0 {
            spans.push(gap());
            spans.extend(row.meter.spans(self.bar));
        }
        spans.push(gap());
        spans.push(Span::styled(
            format_compact!("{:>1$}", row.count, self.count).to_string(),
            row.count_style,
        ));
        spans.push(gap());
        spans.push(Span::styled(
            format_compact!("{:>1$}", row.amount, self.amount).to_string(),
            if row.share.is_empty() { Style::new() } else { working() },
        ));
        if self.share {
            spans.push(gap());
            spans.push(Span::styled(
                format_compact!("{:>SHARE_WIDTH$}", row.share).to_string(),
                dim(),
            ));
        }
        Line::from(spans)
    }
}

fn protocol_row(
    name: &'static str,
    progress: ProtocolProgress,
    checked: bool,
) -> Row {
    if checked {
        let share = (progress.proxies_working as f64)
            / (progress.proxies_checked.max(1) as f64)
            * 100.0_f64;
        Row {
            name,
            meter: Meter {
                done: progress.proxies_checked,
                total: progress.proxies_total,
                working: progress.proxies_working,
            },
            count: format_compact!(
                "{} / {}",
                grouped(progress.proxies_checked),
                grouped(progress.proxies_total)
            ),
            count_style: Style::new(),
            amount: grouped(progress.proxies_working),
            share: format_compact!("{share:.1}%"),
        }
    } else {
        Row {
            name,
            meter: Meter {
                done: progress.sources_scraped,
                total: progress.sources_total,
                working: 0,
            },
            count: format_compact!(
                "{} / {}",
                progress.sources_scraped,
                progress.sources_total
            ),
            count_style: Style::new(),
            amount: grouped(progress.proxies_total),
            share: CompactString::default(),
        }
    }
}

fn database_row(name: &'static str, progress: DbProgress) -> Row {
    let (count, count_style) = match progress.finished_ok {
        Some(false) => (CompactString::const_new("failed"), warning()),
        _ if progress.downloaded == 0 => {
            (CompactString::const_new("waiting"), dim())
        }
        _ if progress.total == 0 => (
            format_compact!("{:.1} MB", megabytes(progress.downloaded)),
            Style::new(),
        ),
        _ => (
            format_compact!(
                "{:.1} / {:.1} MB",
                megabytes(progress.downloaded),
                megabytes(progress.total)
            ),
            Style::new(),
        ),
    };
    Row {
        name,
        meter: Meter {
            done: progress.downloaded,
            total: progress.total,
            working: 0,
        },
        count,
        count_style,
        amount: CompactString::default(),
        share: CompactString::default(),
    }
}

// A database needs attention only until it is ready, or if it never will be.
fn database_rows(config: &Config, state: &State) -> Vec<Row> {
    [
        ("ASN", config.asn_enabled(), state.asn_db),
        ("Geo", config.geolocation_enabled(), state.geo_db),
    ]
    .into_iter()
    .filter(|&(_, enabled, progress)| {
        enabled
            && progress
                .finished_ok
                .map_or(matches!(state.mode, Mode::Running), |ok| !ok)
    })
    .map(|(name, _, progress)| database_row(name, progress))
    .collect()
}

fn stepper(config: &Config, state: &State) -> Vec<Span<'static>> {
    let mut steps = vec![(Stage::Scraping, "Scrape")];
    if config.checking.check_url.is_some() {
        steps.push((Stage::Checking, "Check"));
    }
    steps.push((Stage::Saving, "Save"));

    let finished = matches!(state.mode, Mode::Done | Mode::Quit);
    let mut spans = Vec::with_capacity(steps.len().saturating_mul(2));
    for (stage, name) in steps {
        if !spans.is_empty() {
            spans.push(Span::styled(
                format_compact!(" {TRACK} ").to_string(),
                dim(),
            ));
        }
        let style = if finished || stage < state.stage {
            Style::new()
        } else if stage == state.stage {
            bold()
        } else {
            dim()
        };
        spans.push(Span::styled(name, style));
    }
    spans
}

fn timing(config: &Config, state: &State) -> Vec<Span<'static>> {
    let elapsed = clock(state.elapsed_seconds());
    match state.mode {
        Mode::Done | Mode::Quit => {
            let verb =
                if state.stopped { "Stopped after" } else { "Finished in" };
            vec![Span::raw(format_compact!("{verb} {elapsed}").to_string())]
        }
        Mode::Stopping => vec![
            Span::styled("Stopping", bold()),
            gap(),
            Span::raw(elapsed.to_string()),
        ],
        Mode::Running => {
            let mut spans = Vec::new();
            if let Some(rate) = state.checks_per_second() {
                spans.push(Span::raw(grouped(rate).to_string()));
                spans.push(Span::styled(" checks/s", dim()));
                spans.push(gap());
            }
            spans.push(Span::raw(elapsed.to_string()));
            if let Some(left) = state.seconds_left(config) {
                spans.push(gap());
                spans.push(Span::raw(
                    format_compact!("~{}", clock(left)).to_string(),
                ));
                spans.push(Span::styled(" left", dim()));
            }
            spans
        }
    }
}

fn header(config: &Config, state: &State, width: usize) -> Line<'static> {
    let right = timing(config, state);
    let steps = stepper(config, state);

    let right_width = line_width(&right);
    let steps_width = line_width(&steps).saturating_add(COLUMN_GAP);
    let title_width = TITLE.len().saturating_add(COLUMN_GAP);
    let mut used = right_width;

    let mut left = Vec::new();
    if used.saturating_add(steps_width).saturating_add(title_width) <= width {
        left.push(Span::styled(TITLE, bold()));
        left.push(gap());
        left.push(gap());
        used = used.saturating_add(title_width).saturating_add(COLUMN_GAP);
    }
    if used.saturating_add(steps_width) <= width {
        used = used.saturating_add(steps_width);
        left.extend(steps);
        left.push(gap());
    }

    left.push(Span::raw(" ".repeat(width.saturating_sub(used))));
    left.extend(right);
    Line::from(left)
}

fn outcome(config: &Config, state: &State) -> Option<Line<'static>> {
    if !matches!(state.mode, Mode::Done) {
        return None;
    }
    Some(match state.saved {
        Some(1) => Line::from(format!(
            "Saved 1 proxy to {}",
            config.output.path.display()
        )),
        Some(saved) => Line::from(format!(
            "Saved {} proxies to {}",
            grouped(saved),
            config.output.path.display()
        )),
        None => Line::styled(
            "Nothing to save, the previous results were kept",
            warning(),
        ),
    })
}

fn overview(
    config: &Config,
    state: &State,
    width: usize,
    roomy: bool,
) -> Text<'static> {
    let checked =
        config.checking.check_url.is_some() && state.stage >= Stage::Checking;
    let headings = if checked {
        Headings { count: "Checked", amount: "Working" }
    } else {
        Headings { count: "Sources", amount: "Found" }
    };

    let enabled = config.scraping.enabled_protocols;
    let mut protocols: Vec<_> = enabled
        .iter()
        .map(|protocol| {
            protocol_row(
                protocol.as_str_uppercase(),
                *state.protocols.get(protocol),
                checked,
            )
        })
        .collect();
    if protocols.len() > 1 {
        let all = enabled
            .iter()
            .map(|protocol| *state.protocols.get(protocol))
            .fold(ProtocolProgress::default(), ProtocolProgress::plus);
        protocols.push(protocol_row("All", all, checked));
    }
    let databases = database_rows(config, state);
    let columns =
        Columns::fit(width, protocols.iter().chain(&databases), &headings);

    let mut lines = vec![header(config, state, width)];
    if roomy {
        lines.push(Line::default());
        lines.push(columns.heading(&headings));
    }
    lines.extend(protocols.iter().map(|row| columns.row(row)));
    if !databases.is_empty() {
        if roomy {
            lines.push(Line::default());
        }
        lines.extend(databases.iter().map(|row| columns.row(row)));
    }
    if let Some(outcome) = outcome(config, state) {
        if roomy {
            lines.push(Line::default());
        }
        lines.push(outcome);
    }
    if roomy {
        lines.push(Line::default());
    }
    Text::from(lines)
}

fn render_chart(f: &mut Frame<'_>, area: Rect, state: &State) {
    let [title_area, bars_area] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(CHART_BARS_HEIGHT),
    ])
    .areas(area);

    let shown = state
        .found_per_second
        .len()
        .saturating_sub(usize::from(bars_area.width));
    let found = state.found_per_second.get(shown..).unwrap_or_default();
    let peak = found.iter().copied().max().unwrap_or_default();

    f.render_widget(
        Line::styled("Working proxies found per second", dim()),
        title_area,
    );
    f.render_widget(
        Line::styled(format!("peak {peak}"), dim()).right_aligned(),
        title_area,
    );
    f.render_widget(
        Sparkline::default().data(found).style(working()),
        bars_area,
    );
}

fn footer(
    state: &State,
    width: usize,
) -> (Line<'static>, Option<Line<'static>>) {
    if state.input_lost {
        return (
            Line::styled(
                "Keyboard input was lost, the program exits when the run ends",
                warning(),
            ),
            None,
        );
    }

    let mut keys: Vec<(&str, &str)> = Vec::with_capacity(4);
    if matches!(state.mode, Mode::Running) {
        keys.push(("q", "stop"));
        keys.push(("ctrl-c", "quit"));
    } else {
        keys.push(("q", "quit"));
    }
    keys.push(("\u{2191}\u{2193}", "scroll logs"));
    keys.push(("end", "latest"));

    let mut spans = Vec::with_capacity(keys.len().saturating_mul(4));
    for (key, action) in keys {
        let entry = [
            gap(),
            Span::styled(key, bold()),
            Span::styled(format_compact!(" {action}").to_string(), dim()),
        ];
        let needed = line_width(&spans).saturating_add(line_width(&entry));
        if needed > width {
            break;
        }
        spans.extend(entry);
    }
    if !spans.is_empty() {
        spans.remove(0);
    }

    let keys_width = line_width(&spans);
    let url = (keys_width.saturating_add(URL.len()).saturating_add(COLUMN_GAP)
        <= width)
        .then(|| Line::styled(URL, dim()).right_aligned());
    (Line::from(spans), url)
}

fn wrap(text: &str, width: usize) -> impl Iterator<Item = &str> {
    let mut rest = text;
    std::iter::from_fn(move || {
        if rest.is_empty() {
            return None;
        }
        let end =
            rest.char_indices().nth(width).map_or(rest.len(), |(end, _)| {
                rest.get(..end)
                    .and_then(|line| line.rfind(' '))
                    .filter(|&space| space >= end.div_euclid(2))
                    .map_or(end, |space| space.saturating_add(1))
            });
        let (line, tail) = rest.split_at(end);
        rest = tail;
        Some(line)
    })
}

struct LogLine;

impl LogFormatter for LogLine {
    fn min_width(&self) -> u16 {
        LOG_MIN_WIDTH
    }

    fn format(&self, width: usize, evt: &ExtLogRecord) -> Vec<Line<'_>> {
        let (prefix, style) = match evt.level {
            log::Level::Error => ("error: ", error()),
            log::Level::Warn => ("warning: ", warning()),
            log::Level::Info => ("", Style::new()),
            log::Level::Debug | log::Level::Trace => ("", dim()),
        };

        // The tracing layer puts a space in front of every message.
        let text = format!("{prefix}{}", evt.msg().trim_start());
        let mut lines = Vec::new();
        for paragraph in text.lines() {
            for part in wrap(paragraph, width.saturating_sub(LOG_INDENT)) {
                let margin = if lines.is_empty() {
                    format!(
                        "{:<LOG_INDENT$}",
                        evt.timestamp.strftime("%H:%M:%S").to_string()
                    )
                } else {
                    " ".repeat(LOG_INDENT)
                };
                lines.push(Line::from(vec![
                    Span::styled(margin, dim()),
                    Span::styled(part.to_owned(), style),
                ]));
            }
        }
        lines
    }
}

pub fn draw(
    f: &mut Frame<'_>,
    config: &Config,
    state: &State,
    logger_state: &TuiWidgetState,
) {
    let full = f.area();
    let roomy = full.height >= ROOMY_HEIGHT;
    let area = full.inner(Margin::new(1, u16::from(roomy)));
    let chart_shown = full.height >= CHART_MIN_HEIGHT
        && state.found_per_second.iter().any(|&found| found != 0);

    let overview = overview(config, state, usize::from(area.width), roomy);
    let [overview_area, chart_area, logs_area, _, footer_area] =
        Layout::vertical([
            Constraint::Length(
                u16::try_from(overview.height()).unwrap_or(u16::MAX),
            ),
            Constraint::Length(if chart_shown { CHART_HEIGHT } else { 0 }),
            Constraint::Fill(1),
            Constraint::Length(u16::from(roomy)),
            Constraint::Length(1),
        ])
        .areas(area);

    f.render_widget(Paragraph::new(overview), overview_area);
    if chart_shown {
        render_chart(f, chart_area, state);
    }
    f.render_widget(
        TuiLoggerWidget::default()
            .formatter(Box::new(LogLine))
            .state(logger_state),
        logs_area,
    );

    let (keys, url) = footer(state, usize::from(footer_area.width));
    f.render_widget(keys, footer_area);
    if let Some(url) = url {
        f.render_widget(url, footer_area);
    }
}

#[cfg(test)]
mod tests {
    use ratatui::{Terminal, backend::TestBackend, style::Color, text::Span};
    use tui_logger::TuiWidgetState;

    use super::{Meter, clock, database_row, draw, grouped, scale, wrap};
    use crate::{
        config::{Config, tests::test_config},
        event::Stage,
        proxy::ProxyType,
        tui::app::{DbProgress, Mode, ProtocolProgress, State},
    };

    const MB: usize = 1_000_000;

    fn render(
        config: &Config,
        state: &State,
        width: u16,
        height: u16,
    ) -> String {
        let mut terminal =
            Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|f| draw(f, config, state, &TuiWidgetState::default()))
            .unwrap();
        let buffer = terminal.backend().buffer();
        buffer
            .content()
            .chunks(usize::from(width.max(1)))
            .map(|row| {
                row.iter()
                    .map(ratatui::buffer::Cell::symbol)
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn checking_state() -> State {
        let mut state = State {
            stage: Stage::Checking,
            ticks: 1200,
            checking_since: 1100,
            proxies_checked: 12_480,
            ..State::default()
        };
        *state.protocols.get_mut(ProxyType::Http) = ProtocolProgress {
            sources_total: 21,
            sources_scraped: 21,
            proxies_total: 40_213,
            proxies_checked: 12_480,
            proxies_working: 312,
        };
        state
    }

    fn text(spans: &[Span<'_>]) -> String {
        spans.iter().map(|span| span.content.as_ref()).collect()
    }

    fn meter(done: usize, total: usize, working: usize) -> Meter {
        Meter { done, total, working }
    }

    #[test]
    fn numbers_read_in_groups_of_three() {
        assert_eq!(grouped(0), "0");
        assert_eq!(grouped(999), "999");
        assert_eq!(grouped(1000), "1,000");
        assert_eq!(grouped(12_480), "12,480");
        assert_eq!(grouped(1_234_567), "1,234,567");
    }

    #[test]
    fn durations_grow_an_hours_field_only_when_needed() {
        assert_eq!(clock(0), "0:00");
        assert_eq!(clock(134), "2:14");
        assert_eq!(clock(3_725), "1:02:05");
    }

    #[test]
    fn scale_survives_nothing_to_measure_and_overshoot() {
        assert_eq!(scale(1, 0, 10), 0);
        assert_eq!(scale(11, 10, 10), 10);
        assert_eq!(scale(usize::MAX, usize::MAX, 10), 10);
    }

    #[test]
    fn meter_steps_in_half_cells() {
        assert_eq!(text(&meter(0, 8, 0).spans(4)), "\u{2500}".repeat(4));
        assert_eq!(
            text(&meter(3, 8, 0).spans(4)),
            "\u{2501}\u{257e}\u{2500}\u{2500}"
        );
        assert_eq!(
            text(&meter(4, 8, 0).spans(4)),
            "\u{2501}\u{2501}\u{2500}\u{2500}"
        );
    }

    #[test]
    fn meter_is_full_only_when_everything_is_done() {
        assert!(text(&meter(999, 1000, 0).spans(40)).ends_with('\u{257e}'));
        assert!(!text(&meter(1000, 1000, 0).spans(40)).contains('\u{2500}'));
    }

    #[test]
    fn meter_never_rounds_the_working_proxies_away() {
        let spans = meter(500, 1000, 1).spans(40);
        let passed = spans.first().unwrap();
        assert_eq!(passed.style.fg, Some(Color::Green));
        assert_eq!(passed.content.chars().count(), 1);
    }

    #[test]
    fn meter_stays_the_requested_width() {
        for done in 0..=100 {
            let spans = meter(done, 100, done.div_euclid(7)).spans(33);
            assert_eq!(text(&spans).chars().count(), 33);
        }
    }

    #[test]
    fn database_row_says_where_the_download_is() {
        let count = |finished_ok, downloaded, total| {
            database_row("ASN", DbProgress { finished_ok, downloaded, total })
                .count
        };
        assert_eq!(count(Some(false), MB, MB), "failed");
        assert_eq!(count(None, 0, 0), "waiting");
        assert_eq!(count(None, MB, 0), "1.0 MB");
        assert_eq!(count(None, MB, 2 * MB), "1.0 / 2.0 MB");
    }

    #[test]
    fn wrap_prefers_spaces_and_still_breaks_long_words() {
        let lines: Vec<_> = wrap("one two three", 8).collect();
        assert_eq!(lines, ["one two ", "three"]);

        let lines: Vec<_> = wrap("abcdefgh", 3).collect();
        assert_eq!(lines, ["abc", "def", "gh"]);
    }

    #[test]
    fn wrap_does_not_strand_a_word_before_a_very_long_one() {
        let lines: Vec<_> =
            wrap("warning: https://example.com/path", 24).collect();
        assert_eq!(lines, ["warning: https://example", ".com/path"]);
    }

    #[tokio::test]
    async fn several_protocols_get_a_total_and_one_does_not() {
        let config = test_config(|_| {}).await;
        let mut state = checking_state();
        state.protocols.get_mut(ProxyType::Socks4).proxies_working = 8;

        let screen = render(&config, &state, 120, 30);
        assert!(screen.contains("All"), "{screen}");
        assert!(screen.contains("320"), "{screen}");

        let config = test_config(|raw| {
            raw.scraping.socks4.enabled = false;
            raw.scraping.socks5.enabled = false;
        })
        .await;
        let screen = render(&config, &state, 120, 30);
        assert!(!screen.contains("All"), "{screen}");
    }

    #[tokio::test]
    async fn the_chart_waits_for_something_to_show() {
        let config = test_config(|_| {}).await;
        let mut state = checking_state();

        state.found_per_second = vec![0, 0];
        let screen = render(&config, &state, 120, 30);
        assert!(!screen.contains("per second"), "{screen}");

        state.found_per_second = vec![0, 7];
        let screen = render(&config, &state, 120, 30);
        assert!(screen.contains("peak 7"), "{screen}");
    }

    #[tokio::test]
    async fn a_key_that_does_not_fit_is_left_out_whole() {
        let config = test_config(|_| {}).await;
        let screen = render(&config, &State::default(), 40, 30);
        assert!(screen.contains("scroll logs"), "{screen}");
        assert!(!screen.contains("end"), "{screen}");
    }

    #[tokio::test]
    async fn scraping_counts_sources_and_found_proxies() {
        let config = test_config(|_| {}).await;
        let mut state = State::default();
        *state.protocols.get_mut(ProxyType::Socks5) = ProtocolProgress {
            sources_total: 23,
            sources_scraped: 9,
            proxies_total: 38_214,
            ..ProtocolProgress::default()
        };

        let screen = render(&config, &state, 100, 30);
        assert!(screen.contains("Sources"), "{screen}");
        assert!(screen.contains("9 / 23"), "{screen}");
        assert!(screen.contains("38,214"), "{screen}");
        assert!(!screen.contains("Working"), "{screen}");
    }

    #[tokio::test]
    async fn checking_shows_the_yield_speed_and_time_left() {
        let config = test_config(|_| {}).await;
        let screen = render(&config, &checking_state(), 120, 30);

        assert!(screen.contains("12,480 / 40,213"), "{screen}");
        assert!(screen.contains("312"), "{screen}");
        assert!(screen.contains("2.5%"), "{screen}");
        assert!(screen.contains("1,248 checks/s"), "{screen}");
        assert!(screen.contains("2:00"), "{screen}");
        assert!(screen.contains("~0:23"), "{screen}");
    }

    #[tokio::test]
    async fn without_a_check_url_nothing_mentions_checking() {
        let config = test_config(|raw| raw.checking.check_url = None).await;
        let state = State { stage: Stage::Saving, ..State::default() };

        let screen = render(&config, &state, 100, 30);
        assert!(!screen.contains("Check"), "{screen}");
        assert!(!screen.contains("Working"), "{screen}");
        assert!(screen.contains("Found"), "{screen}");
    }

    #[tokio::test]
    async fn a_failed_database_stays_and_a_ready_one_goes() {
        let config = test_config(|_| {}).await;
        let mut state = State::default();
        state.asn_db.finished_ok = Some(true);
        state.geo_db.finished_ok = Some(false);

        let screen = render(&config, &state, 100, 30);
        assert!(!screen.contains("ASN"), "{screen}");
        assert!(screen.contains("Geo"), "{screen}");
        assert!(screen.contains("failed"), "{screen}");
    }

    #[tokio::test]
    async fn the_end_of_a_run_says_what_happened() {
        let config = test_config(|_| {}).await;
        let mut state = checking_state();
        state.mode = Mode::Done;
        state.stage = Stage::Saving;

        state.saved = Some(1_517);
        let screen = render(&config, &state, 100, 30);
        assert!(screen.contains("Finished in 2:00"), "{screen}");
        assert!(screen.contains("Saved 1,517 proxies to "), "{screen}");
        assert!(screen.contains("q quit"), "{screen}");

        state.saved = Some(1);
        state.stopped = true;
        let screen = render(&config, &state, 100, 30);
        assert!(screen.contains("Stopped after 2:00"), "{screen}");
        assert!(screen.contains("Saved 1 proxy to "), "{screen}");

        state.saved = None;
        let screen = render(&config, &state, 100, 30);
        assert!(screen.contains("previous results were kept"), "{screen}");
    }

    #[tokio::test]
    async fn narrow_terminals_lose_the_percentage_before_the_bar() {
        let config = test_config(|_| {}).await;
        let state = checking_state();

        let screen = render(&config, &state, 50, 30);
        assert!(!screen.contains("2.5%"), "{screen}");
        assert!(screen.contains('\u{2501}'), "{screen}");

        let screen = render(&config, &state, 40, 30);
        assert!(!screen.contains('\u{2501}'), "{screen}");
        assert!(screen.contains("12,480 / 40,213"), "{screen}");
    }

    #[tokio::test]
    async fn any_terminal_size_renders() {
        let config = test_config(|_| {}).await;
        let mut stopping = checking_state();
        stopping.mode = Mode::Stopping;
        let mut done = checking_state();
        done.mode = Mode::Done;
        done.input_lost = true;

        for state in [&State::default(), &checking_state(), &stopping, &done] {
            for width in (1..140).step_by(3) {
                for height in 1..40 {
                    render(&config, state, width, height);
                }
            }
        }
    }
}
