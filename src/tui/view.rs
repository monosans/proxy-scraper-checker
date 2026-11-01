use std::time::Duration;

use compact_str::{CompactString, ToCompactString as _, format_compact};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Margin},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::Paragraph,
};
use tui_logger::{ExtLogRecord, LogFormatter, TuiLoggerWidget, TuiWidgetState};
use unicode_segmentation::UnicodeSegmentation as _;
use unicode_width::UnicodeWidthStr as _;

use super::app::{DbProgress, FASTEST_KEPT, Mode, ProtocolProgress, State};
use crate::{
    config::Config,
    event::Stage,
    utils::{clock, grouped},
};

const NAME_WIDTH: usize = 6;
const SHARE_WIDTH: usize = 6;
const GAP: &str = "  ";
const SLASH: &str = " / ";
const MIN_BAR_WIDTH: usize = 10;
const LOG_INDENT: usize = 10;
const LOG_INDENT_NARROW: usize = 2;
const LOG_MIN_WIDTH: u16 = 18;
// Below this the timestamps go to the messages.
const LOG_TIME_MIN_WIDTH: usize = 60;
// Below this the blank lines and the column headings go to the logs.
const ROOMY_HEIGHT: u16 = 20;
// A phone held upright: too narrow for a bar beside the numbers, but tall
// enough to give each row a second line for one across the whole screen. The
// decision rests on the screen alone, so the layout does not flip mid-run as
// the numbers grow.
const STACKED_MAX_WIDTH: u16 = 64;
const STACKED_MIN_HEIGHT: u16 = 24;
// The fastest proxies only take what the logs can spare, and a list cut down
// to one or two lines would say nothing. Once the run is over, they are worth
// more than the logs are.
const LOGS_MIN_HEIGHT: u16 = 6;
const LOGS_MIN_HEIGHT_DONE: u16 = 3;
const FASTEST_MIN_ROWS: u16 = 3;
const FASTEST_TITLE_AND_GAP: u16 = 2;
// Any further apart and the eye loses the row between address and latency.
const FASTEST_MAX_WIDTH: usize = 40;
const LATENCY_WIDTH: usize = 7;
const TITLE: &str = "proxy-scraper-checker";
// Set aside for the timing as a whole, so the title does not blink as the
// numbers change width.
const HEADER_TIMING_WIDTH: usize = 40;
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
    Span::raw(GAP)
}

fn line_width(spans: &[Span<'_>]) -> usize {
    spans.iter().map(Span::width).sum()
}

fn fits(spans: &[Span<'_>], entry: &[Span<'_>], width: usize) -> bool {
    line_width(spans).saturating_add(line_width(entry)) <= width
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
    done: CompactString,
    total: CompactString,
    aligned: bool,
    count_style: Style,
    amount: CompactString,
    amount_style: Style,
    share: CompactString,
}

impl Row {
    fn joined_count(&self) -> CompactString {
        format_compact!("{}{SLASH}{}", self.done, self.total)
    }
}

struct Headings {
    count: &'static str,
    amount: &'static str,
    amount_style: Style,
}

#[derive(Default)]
struct Columns {
    name: usize,
    bar: usize,
    bar_below: usize,
    count: usize,
    total: usize,
    short: bool,
    amount: usize,
    share: bool,
}

impl Columns {
    // Numbers keep their full width and the bar takes what is left; on narrow
    // terminals the percentage goes first, then the bar. A stacked layout gives
    // the bar its own line instead, and the spare room pushes the numbers to
    // the end of it. If the numbers alone do not fit, the totals go.
    fn fit<'a>(
        width: usize,
        rows: impl Iterator<Item = &'a Row>,
        headings: &Headings,
        stacked: bool,
    ) -> Self {
        let mut columns = Self {
            name: NAME_WIDTH,
            count: headings.count.len(),
            amount: headings.amount.len(),
            ..Self::default()
        };
        // The done part is as wide as the total it is heading for, so the
        // slashes line up and stay put while it grows.
        let mut done = 0;
        let mut alone = columns.count;
        let mut count_short = columns.count;
        let mut any_share = false;
        for row in rows {
            if row.total.is_empty() {
                alone = alone.max(row.done.len());
            } else if row.aligned {
                done = done.max(row.done.len()).max(row.total.len());
                columns.total = columns.total.max(row.total.len());
            } else {
                alone = alone.max(row.joined_count().len());
            }
            count_short = count_short.max(row.done.len());
            columns.amount = columns.amount.max(row.amount.len());
            any_share |= !row.share.is_empty();
        }

        columns.count = alone.max(
            done.saturating_add(SLASH.len()).saturating_add(columns.total),
        );

        let numbers = move |count: usize| {
            [NAME_WIDTH, count, columns.amount, GAP.len().saturating_mul(2)]
                .into_iter()
                .sum::<usize>()
        };
        columns.short = numbers(columns.count) > width;
        if columns.short {
            columns.count = count_short;
        }
        let room = width.saturating_sub(numbers(columns.count));
        let share_width = SHARE_WIDTH.saturating_add(GAP.len());

        if stacked {
            columns.share = any_share && room >= share_width;
            columns.name = NAME_WIDTH.saturating_add(if columns.share {
                room.saturating_sub(share_width)
            } else {
                room
            });
            columns.bar_below = width;
            return columns;
        }

        let room = room.saturating_sub(GAP.len());
        let room_with_share = room.saturating_sub(share_width);
        columns.share = any_share && room_with_share >= MIN_BAR_WIDTH;
        columns.bar = if columns.share { room_with_share } else { room };
        if columns.bar < MIN_BAR_WIDTH {
            columns.bar = 0;
        }
        columns
    }

    fn heading(&self, headings: &Headings) -> Line<'static> {
        let mut spans = vec![Span::raw(" ".repeat(self.name))];
        if self.bar != 0 {
            spans.push(gap());
            spans.push(Span::raw(" ".repeat(self.bar)));
        }
        spans.push(gap());
        spans.push(Span::styled(
            format!("{:>1$}", headings.count, self.count),
            dim(),
        ));
        spans.push(gap());
        spans.push(Span::styled(
            format!("{:>1$}", headings.amount, self.amount),
            headings.amount_style,
        ));
        Line::from(spans)
    }

    fn row(&self, row: &Row) -> Line<'static> {
        let mut spans = vec![Span::raw(format!("{:<1$}", row.name, self.name))];
        if self.bar != 0 {
            spans.push(gap());
            spans.extend(row.meter.spans(self.bar));
        }
        spans.push(gap());
        spans.push(Span::styled(self.count_cell(row), row.count_style));
        spans.push(gap());
        spans.push(Span::styled(
            format!("{:>1$}", row.amount, self.amount),
            row.amount_style,
        ));
        if self.share {
            spans.push(gap());
            spans.push(Span::styled(
                format!("{:>SHARE_WIDTH$}", row.share),
                dim(),
            ));
        }
        Line::from(spans)
    }

    fn count_cell(&self, row: &Row) -> String {
        if self.short || row.total.is_empty() {
            return format!("{:>1$}", row.done, self.count);
        }
        if !row.aligned {
            return format!("{:>1$}", row.joined_count(), self.count);
        }
        let done_width =
            self.count.saturating_sub(self.total).saturating_sub(SLASH.len());
        format!("{:>done_width$}{SLASH}{:>2$}", row.done, row.total, self.total)
    }

    fn lines(&self, rows: &[Row]) -> Vec<Line<'static>> {
        let mut lines = Vec::with_capacity(rows.len().saturating_mul(2));
        for row in rows {
            lines.push(self.row(row));
            if self.bar_below != 0 {
                lines.push(Line::from(row.meter.spans(self.bar_below)));
            }
        }
        lines
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
            done: grouped(progress.proxies_checked),
            total: grouped(progress.proxies_total),
            aligned: true,
            count_style: Style::new(),
            amount: grouped(progress.proxies_working),
            amount_style: working(),
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
            done: progress.sources_scraped.to_compact_string(),
            total: progress.sources_total.to_compact_string(),
            aligned: true,
            count_style: Style::new(),
            amount: grouped(progress.proxies_total),
            amount_style: Style::new(),
            share: CompactString::default(),
        }
    }
}

fn database_row(name: &'static str, progress: DbProgress) -> Row {
    let (done, total, count_style) = match progress.finished_ok {
        Some(false) => ("failed".into(), CompactString::default(), warning()),
        _ if progress.downloaded == 0 => {
            ("waiting".into(), CompactString::default(), dim())
        }
        _ if progress.total == 0 => (
            format_compact!("{:.1} MB", megabytes(progress.downloaded)),
            CompactString::default(),
            Style::new(),
        ),
        _ => (
            format_compact!("{:.1} MB", megabytes(progress.downloaded)),
            format_compact!("{:.1} MB", megabytes(progress.total)),
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
        done,
        total,
        // Megabytes have nothing in common with the counts above them.
        aligned: false,
        count_style,
        amount: CompactString::default(),
        amount_style: Style::new(),
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
    .filter(move |&(_, enabled, progress)| {
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
            spans.push(Span::styled(format!(" {TRACK} "), dim()));
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

// The clock always stays; the time left goes after the speed.
fn timing(config: &Config, state: &State, width: usize) -> Vec<Span<'static>> {
    let elapsed = clock(state.elapsed_seconds());
    match state.mode {
        Mode::Done | Mode::Quit => {
            let verb =
                if state.stopped { "Stopped after" } else { "Finished in" };
            vec![Span::raw(format!("{verb} {elapsed}"))]
        }
        Mode::Stopping => vec![
            Span::styled("Stopping", bold()),
            gap(),
            Span::raw(elapsed.to_string()),
        ],
        Mode::Running => {
            let mut spans = vec![Span::raw(elapsed.to_string())];
            if let Some(left) = state.seconds_left(config) {
                let entry = [
                    gap(),
                    Span::raw(format!("~{}", clock(left))),
                    Span::styled(" left", dim()),
                ];
                if fits(&spans, &entry, width) {
                    spans.extend(entry);
                }
            }
            if let Some(rate) = state.checks_per_second() {
                let entry = [
                    Span::raw(grouped(rate).to_string()),
                    Span::styled(" checks/s", dim()),
                    gap(),
                ];
                if fits(&spans, &entry, width) {
                    spans.splice(0..0, entry);
                }
            }
            spans
        }
    }
}

fn header(
    config: &Config,
    state: &State,
    width: usize,
    stacked: bool,
) -> Vec<Line<'static>> {
    let steps = stepper(config, state);

    if stacked {
        let right = timing(config, state, width);
        return vec![Line::from(steps), Line::from(right)];
    }

    let steps_width = line_width(&steps).saturating_add(GAP.len());
    // Steps that can't fit beside the clock go, and leave their room to it.
    let beside_steps = width.saturating_sub(steps_width);
    let mut right = timing(config, state, beside_steps);
    if line_width(&right) > beside_steps {
        right = timing(config, state, width);
    }
    let right_width = line_width(&right);
    let title_width = TITLE.len().saturating_add(GAP.len().saturating_mul(2));
    let mut used = right_width;

    let mut left = Vec::new();
    if HEADER_TIMING_WIDTH
        .saturating_add(steps_width)
        .saturating_add(title_width)
        <= width
    {
        left.push(Span::styled(TITLE, bold()));
        left.push(gap());
        left.push(gap());
        used = used.saturating_add(title_width);
    }
    if used.saturating_add(steps_width) <= width {
        used = used.saturating_add(steps_width);
        left.extend(steps);
        left.push(gap());
    }

    left.push(Span::raw(" ".repeat(width.saturating_sub(used))));
    left.extend(right);
    vec![Line::from(left)]
}

// Android paths are long, and this one is what the whole run was for.
fn outcome(config: &Config, state: &State, width: usize) -> Vec<Line<'static>> {
    if !matches!(state.mode, Mode::Done) {
        return Vec::new();
    }
    let (text, style) = match state.saved {
        Some(1) => (
            format!("Saved 1 proxy to {}", config.output.path.display()),
            Style::new(),
        ),
        Some(saved) => (
            format!(
                "Saved {} proxies to {}",
                grouped(saved),
                config.output.path.display()
            ),
            Style::new(),
        ),
        None => (
            "Nothing to save, the previous results were kept".to_owned(),
            warning(),
        ),
    };
    wrap(&text, width.max(1))
        .map(move |line| Line::styled(line.to_owned(), style))
        .collect()
}

fn overview(
    config: &Config,
    state: &State,
    width: usize,
    roomy: bool,
    stacked: bool,
) -> Text<'static> {
    let checked =
        config.checking.check_url.is_some() && state.stage >= Stage::Checking;
    let headings = if checked {
        Headings {
            count: "Checked",
            amount: "Working",
            amount_style: working(),
        }
    } else {
        Headings { count: "Sources", amount: "Found", amount_style: dim() }
    };

    let enabled = config.scraping.enabled_protocols;
    let mut protocols: Vec<_> = enabled
        .iter()
        .map(move |protocol| {
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
            .map(move |protocol| *state.protocols.get(protocol))
            .fold(ProtocolProgress::default(), ProtocolProgress::plus);
        protocols.push(protocol_row("All", all, checked));
    }
    let databases = database_rows(config, state);
    let columns = Columns::fit(
        width,
        protocols.iter().chain(&databases),
        &headings,
        stacked,
    );

    // The result comes first: if the screen is too short, the table is what
    // gets cut.
    let mut lines = header(config, state, width, stacked);
    lines.extend(outcome(config, state, width));
    if roomy {
        lines.push(Line::default());
        lines.push(columns.heading(&headings));
    }
    lines.extend(columns.lines(&protocols));
    if !databases.is_empty() {
        if roomy {
            lines.push(Line::default());
        }
        lines.extend(columns.lines(&databases));
    }
    if roomy {
        lines.push(Line::default());
    }
    Text::from(lines)
}

fn latency(latency: Duration) -> CompactString {
    let millis = latency.as_millis();
    if millis < 1000 {
        format_compact!("{millis} ms")
    } else {
        format_compact!("{:.2} s", latency.as_secs_f64())
    }
}

fn elided(text: &str, width: usize) -> CompactString {
    if text.chars().count() <= width {
        return text.into();
    }
    let mut elided: CompactString =
        text.chars().take(width.saturating_sub(1)).collect();
    elided.push('\u{2026}');
    elided
}

// A cut-off port would read as a real one, so on narrow screens the protocol
// goes before any address does.
fn fastest(state: &State, width: usize, rows: usize) -> Text<'static> {
    let proxies = state.fastest.iter().take(rows);
    let longest =
        proxies.clone().map(|proxy| proxy.address.len()).max().unwrap_or(0);
    let room = width
        .min(FASTEST_MAX_WIDTH)
        .saturating_sub(GAP.len())
        .saturating_sub(LATENCY_WIDTH);
    let name_width = NAME_WIDTH.saturating_add(GAP.len());
    let named = room >= name_width.saturating_add(longest);
    let room = if named { room.saturating_sub(name_width) } else { room };

    let mut lines = vec![Line::styled("Fastest proxies", dim())];
    for proxy in proxies {
        let mut spans = Vec::with_capacity(4);
        if named {
            spans.push(Span::raw(format!(
                "{:<name_width$}",
                proxy.protocol.as_str_uppercase()
            )));
        }
        spans.push(Span::raw(format!(
            "{:<room$}",
            elided(&proxy.address, room)
        )));
        spans.push(gap());
        spans.push(Span::raw(format!(
            "{:>LATENCY_WIDTH$}",
            latency(proxy.latency)
        )));
        lines.push(Line::from(spans));
    }
    Text::from(lines)
}

fn footer(
    state: &State,
    width: usize,
) -> (Line<'static>, Option<Line<'static>>) {
    if state.input_lost {
        return (
            Line::styled("Keyboard input lost, exits after the run", warning()),
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
        let mut entry = Vec::with_capacity(3);
        if !spans.is_empty() {
            entry.push(gap());
        }
        entry.push(Span::styled(key, bold()));
        entry.push(Span::styled(format!(" {action}"), dim()));
        if !fits(&spans, &entry, width) {
            break;
        }
        spans.extend(entry);
    }

    let keys_width = line_width(&spans);
    let url = (keys_width.saturating_add(URL.len()).saturating_add(GAP.len())
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
        let mut cells = 0_usize;
        let overflow =
            rest.grapheme_indices(true).find(move |&(index, grapheme)| {
                cells = cells.saturating_add(grapheme.width());
                cells > width && index > 0
            });
        let end = overflow.map_or(rest.len(), move |(end, _)| {
            rest.get(..end)
                .and_then(|line| line.rfind(' '))
                .filter(move |&space| space >= end.div_euclid(2))
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
        let timed = width >= LOG_TIME_MIN_WIDTH;
        let indent = if timed { LOG_INDENT } else { LOG_INDENT_NARROW };
        let mut lines = Vec::new();
        for paragraph in text.lines() {
            for part in wrap(paragraph, width.saturating_sub(indent)) {
                let margin = if !lines.is_empty() {
                    " ".repeat(indent)
                } else if timed {
                    format!(
                        "{:<indent$}",
                        evt.timestamp.strftime("%H:%M:%S").to_string()
                    )
                } else {
                    String::new()
                };
                lines.push(Line::from(vec![
                    Span::styled(margin, dim()),
                    Span::styled(part.to_owned(), style),
                ]));
            }
        }
        // The logger takes one line off this count, unchecked.
        if lines.is_empty() {
            lines.push(Line::default());
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
    let stacked =
        full.width < STACKED_MAX_WIDTH && full.height >= STACKED_MIN_HEIGHT;
    let area = full.inner(Margin::new(1, u16::from(roomy)));

    let overview =
        overview(config, state, usize::from(area.width), roomy, stacked);
    let overview_height = u16::try_from(overview.height()).unwrap_or(u16::MAX);
    let fastest_room = area
        .height
        .saturating_sub(overview_height)
        .saturating_sub(u16::from(roomy).saturating_add(1))
        .saturating_sub(if matches!(state.mode, Mode::Done) {
            LOGS_MIN_HEIGHT_DONE
        } else {
            LOGS_MIN_HEIGHT
        })
        .saturating_sub(FASTEST_TITLE_AND_GAP);
    // Compact layouts would show it, only for a taller screen to take it back.
    let fastest_fits = roomy && (stacked || full.width >= STACKED_MAX_WIDTH);
    let fastest_rows = if !fastest_fits || fastest_room < FASTEST_MIN_ROWS {
        0
    } else {
        state.fastest.len().min(FASTEST_KEPT).min(usize::from(fastest_room))
    };
    let fastest_height = if fastest_rows == 0 {
        0
    } else {
        u16::try_from(fastest_rows)
            .unwrap_or(u16::MAX)
            .saturating_add(FASTEST_TITLE_AND_GAP)
    };

    let [overview_area, fastest_area, logs_area, _, footer_area] =
        Layout::vertical([
            Constraint::Length(overview_height),
            Constraint::Length(fastest_height),
            Constraint::Fill(1),
            Constraint::Length(u16::from(roomy)),
            Constraint::Length(1),
        ])
        .areas(area);

    f.render_widget(Paragraph::new(overview), overview_area);
    if fastest_rows != 0 {
        f.render_widget(
            Paragraph::new(fastest(
                state,
                usize::from(area.width),
                fastest_rows,
            )),
            fastest_area,
        );
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
    use std::{path::PathBuf, time::Duration};

    use ratatui::{Terminal, backend::TestBackend, style::Color, text::Span};
    use tui_logger::TuiWidgetState;

    use super::{Meter, database_row, draw, latency, scale, wrap};
    use crate::{
        config::{Config, tests::test_config},
        event::Stage,
        proxy::ProxyType,
        tui::app::{
            DbProgress, FastProxy, Mode, ProtocolProgress, State,
            TICKS_PER_SECOND,
        },
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
            .draw(move |f| draw(f, config, state, &TuiWidgetState::default()))
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
            ticks: 120 * TICKS_PER_SECOND,
            checking_since: 100 * TICKS_PER_SECOND,
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
            let row = database_row(
                "ASN",
                DbProgress { finished_ok, downloaded, total },
            );
            (row.done, row.total)
        };
        assert_eq!(count(Some(false), MB, MB), ("failed".into(), "".into()));
        assert_eq!(count(None, 0, 0), ("waiting".into(), "".into()));
        assert_eq!(count(None, MB, 0), ("1.0 MB".into(), "".into()));
        assert_eq!(count(None, MB, 2 * MB), ("1.0 MB".into(), "2.0 MB".into()));
    }

    #[test]
    fn wrap_prefers_spaces_and_still_breaks_long_words() {
        let lines: Vec<_> = wrap("one two three", 8).collect();
        assert_eq!(lines, ["one two ", "three"]);

        let lines: Vec<_> = wrap("abcdefgh", 3).collect();
        assert_eq!(lines, ["abc", "def", "gh"]);
    }

    #[test]
    fn wrap_counts_the_cells_a_character_takes() {
        let lines: Vec<_> =
            wrap("\u{8def}\u{5f84}\u{8def}\u{5f84}a", 4).collect();
        assert_eq!(lines, ["\u{8def}\u{5f84}", "\u{8def}\u{5f84}", "a"]);

        // Two cells together, one each apart.
        let lines: Vec<_> = wrap("#\u{fe0f}abc", 4).collect();
        assert_eq!(lines, ["#\u{fe0f}ab", "c"]);

        // Too narrow for even one of them, and it still has to end.
        let lines: Vec<_> = wrap("\u{8def}\u{5f84}", 1).collect();
        assert_eq!(lines, ["\u{8def}", "\u{5f84}"]);
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
    async fn the_counts_line_up_on_the_slash() {
        let config = test_config(|_| {}).await;
        let mut state = State { stage: Stage::Checking, ..State::default() };
        for (protocol, count) in [
            (ProxyType::Http, 2_714),
            (ProxyType::Socks4, 330),
            (ProxyType::Socks5, 8_858),
        ] {
            *state.protocols.get_mut(protocol) = ProtocolProgress {
                proxies_total: count,
                proxies_checked: count,
                ..ProtocolProgress::default()
            };
        }

        let screen = render(&config, &state, 100, 30);
        let slashes: Vec<_> =
            screen.lines().filter_map(|line| line.find(" / ")).collect();
        assert_eq!(slashes.len(), 4, "{screen}");
        assert!(slashes.windows(2).all(|pair| pair.first() == pair.last()));
        assert!(screen.contains("  330 /    330"), "{screen}");
    }

    #[tokio::test]
    async fn megabytes_do_not_widen_the_counts() {
        let config = test_config(|_| {}).await;
        let mut state = State::default();
        *state.protocols.get_mut(ProxyType::Http) = ProtocolProgress {
            sources_total: 23,
            sources_scraped: 9,
            ..ProtocolProgress::default()
        };
        state.asn_db =
            DbProgress { finished_ok: None, downloaded: MB, total: 2 * MB };

        let screen = render(&config, &state, 100, 30);
        assert!(screen.contains("1.0 MB / 2.0 MB"), "{screen}");
        assert!(screen.contains("  9 / 23"), "{screen}");
        assert!(!screen.contains("  9 /  "), "{screen}");
    }

    #[test]
    fn latency_reads_in_the_unit_that_suits_it() {
        assert_eq!(latency(Duration::from_millis(312)), "312 ms");
        assert_eq!(latency(Duration::from_millis(1_240)), "1.24 s");
    }

    fn fast(address: &str, millis: u64) -> FastProxy {
        FastProxy {
            protocol: ProxyType::Socks5,
            address: address.into(),
            latency: Duration::from_millis(millis),
        }
    }

    #[tokio::test]
    async fn the_fastest_proxies_appear_once_there_are_any() {
        let config = test_config(|_| {}).await;
        let mut state = checking_state();

        let screen = render(&config, &state, 100, 30);
        assert!(!screen.contains("Fastest"), "{screen}");

        state.fastest = vec![fast("103.152.112.162:1080", 312)];
        let screen = render(&config, &state, 100, 30);
        assert!(screen.contains("Fastest proxies"), "{screen}");
        assert!(screen.contains("SOCKS5  103.152.112.162:1080"), "{screen}");
        assert!(screen.contains("312 ms"), "{screen}");
    }

    #[tokio::test]
    async fn the_logs_keep_their_room_on_short_screens() {
        let config = test_config(|_| {}).await;
        let state =
            State { fastest: vec![fast("1.1.1.1:80", 5)], ..checking_state() };

        let screen = render(&config, &state, 100, 20);
        assert!(!screen.contains("Fastest"), "{screen}");
    }

    #[tokio::test]
    async fn a_long_host_gives_way_to_the_latency() {
        let config = test_config(|_| {}).await;
        let state = State {
            fastest: vec![fast(&format!("{}.example:80", "a".repeat(60)), 5)],
            ..checking_state()
        };

        let screen = render(&config, &state, 50, 40);
        assert!(screen.contains("\u{2026}"), "{screen}");
        assert!(screen.contains("5 ms"), "{screen}");
    }

    #[tokio::test]
    async fn more_height_never_takes_the_fastest_proxies_away() {
        let config = test_config(|_| {}).await;
        let state = State {
            fastest: vec![fast("1.1.1.1:80", 5); 8],
            ..checking_state()
        };

        for width in [50, 100] {
            let shown: Vec<_> = (1..60)
                .map(|height| {
                    render(&config, &state, width, height).contains("Fastest")
                })
                .collect();
            assert!(shown.is_sorted(), "{width}: {shown:?}");
            assert_eq!(shown.last(), Some(&true));
        }
    }

    #[tokio::test]
    async fn a_finished_run_keeps_its_fastest_proxies_on_a_standard_terminal() {
        let config = test_config(|_| {}).await;
        let state = State {
            mode: Mode::Done,
            saved: Some(8),
            fastest: vec![fast("1.1.1.1:80", 5); 8],
            ..checking_state()
        };

        let screen = render(&config, &state, 80, 24);
        assert!(screen.contains("Fastest"), "{screen}");
    }

    #[tokio::test]
    async fn the_header_keeps_the_steps_and_gives_up_the_title() {
        let config = test_config(|_| {}).await;
        let state = checking_state();

        let screen = render(&config, &state, 80, 24);
        assert!(screen.contains("Scrape"), "{screen}");
        assert!(!screen.contains("proxy-scraper-checker  "), "{screen}");

        let screen = render(&config, &state, 100, 24);
        assert!(screen.contains("Scrape"), "{screen}");
        assert!(screen.contains("proxy-scraper-checker  "), "{screen}");
    }

    #[tokio::test]
    async fn the_steps_stay_when_the_speed_has_to_go() {
        let config = test_config(|_| {}).await;
        let screen = render(&config, &checking_state(), 50, 20);
        assert!(screen.contains("Scrape"), "{screen}");
        assert!(screen.contains(" left"), "{screen}");
        assert!(!screen.contains("checks/s"), "{screen}");
    }

    #[tokio::test]
    async fn the_time_left_stays_when_the_steps_have_gone() {
        let config = test_config(|_| {}).await;
        let screen = render(&config, &checking_state(), 28, 19);
        assert!(!screen.contains("Scrape"), "{screen}");
        assert!(screen.contains(" left"), "{screen}");
    }

    #[tokio::test]
    async fn the_keys_use_the_whole_footer() {
        let config = test_config(|_| {}).await;
        let screen = render(&config, &State::default(), 49, 30);
        assert!(screen.contains("end latest"), "{screen}");
    }

    #[tokio::test]
    async fn a_short_screen_still_says_where_the_results_went() {
        let path = "/data/data/com.termux/files/home/proxy-scraper-checker/out";
        let mut config = test_config(|_| {}).await;
        config.output.path = std::path::PathBuf::from(path);
        let mut state =
            State { mode: Mode::Done, saved: Some(1_517), ..checking_state() };
        state.asn_db.finished_ok = Some(false);
        state.geo_db.finished_ok = Some(false);

        let screen = render(&config, &state, 30, 24);
        let joined = screen.replace(['\n', ' '], "");
        assert!(joined.contains(path), "{screen}");
    }

    #[tokio::test]
    async fn a_narrow_screen_drops_the_protocol_before_cutting_an_address() {
        let config = test_config(|_| {}).await;
        let state = State {
            fastest: vec![fast("103.152.112.162:1080", 312)],
            ..checking_state()
        };

        let screen = render(&config, &state, 34, 40);
        assert!(screen.contains("103.152.112.162:1080"), "{screen}");
        assert!(!screen.contains("SOCKS5  103"), "{screen}");
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
        assert!(screen.contains("624 checks/s"), "{screen}");
        assert!(screen.contains("2:00"), "{screen}");
        assert!(screen.contains("~0:45"), "{screen}");
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

        let screen = render(&config, &state, 50, 20);
        assert!(!screen.contains("2.5%"), "{screen}");
        assert!(screen.contains('\u{2501}'), "{screen}");

        let screen = render(&config, &state, 40, 20);
        assert!(!screen.contains('\u{2501}'), "{screen}");
        assert!(screen.contains("12,480 / 40,213"), "{screen}");
    }

    #[tokio::test]
    async fn an_upright_phone_gets_bars_across_the_whole_screen() {
        let config = test_config(|_| {}).await;
        let mut state = checking_state();
        state.protocols.get_mut(ProxyType::Http).proxies_checked = 20_107;

        for height in [50, 26] {
            let screen = render(&config, &state, 50, height);
            let meter_width = screen
                .lines()
                .map(|line| {
                    line.chars()
                        .filter(|symbol| {
                            "\u{2501}\u{257e}\u{2500}".contains(*symbol)
                        })
                        .count()
                })
                .max();
            assert_eq!(meter_width, Some(48), "{screen}");
            assert!(screen.contains("20,107 / 40,213"), "{screen}");
            assert!(screen.contains("Scrape"), "{screen}");
            assert!(screen.contains("checks/s"), "{screen}");
            assert!(screen.contains('~'), "{screen}");
        }
    }

    #[tokio::test]
    async fn the_layout_does_not_depend_on_the_numbers() {
        let config = test_config(|_| {}).await;
        let mut state = checking_state();
        state.protocols.get_mut(ProxyType::Http).proxies_total = 9;
        state.protocols.get_mut(ProxyType::Http).proxies_checked = 9;

        let screen = render(&config, &state, 63, 30);
        assert!(screen.contains("Scrape"), "{screen}");
        assert!(!screen.contains("  \u{2500}"), "{screen}");

        let screen = render(&config, &state, 64, 30);
        assert!(screen.contains("  \u{2500}"), "{screen}");
    }

    #[tokio::test]
    async fn the_narrowest_phones_keep_the_counts_that_matter() {
        let config = test_config(|_| {}).await;
        let screen = render(&config, &checking_state(), 30, 40);

        assert!(screen.contains("12,480"), "{screen}");
        assert!(!screen.contains("40,213"), "{screen}");
        assert!(screen.contains("Working"), "{screen}");
        assert!(screen.contains("2:00"), "{screen}");
        assert!(screen.contains("~0:45 left"), "{screen}");
        assert!(!screen.contains("checks/s"), "{screen}");
    }

    #[tokio::test]
    async fn a_long_output_path_wraps_instead_of_being_cut() {
        let path = "target/an-output-path-as-long-as-the-ones-on-android";
        let mut config = test_config(|_| {}).await;
        config.output.path = PathBuf::from(path);
        let state =
            State { mode: Mode::Done, saved: Some(1_517), ..checking_state() };

        let screen = render(&config, &state, 30, 40);
        let joined = screen.replace(['\n', ' '], "");
        assert!(joined.contains(path), "{screen}");
    }

    #[tokio::test]
    async fn any_terminal_size_renders() {
        let config = test_config(|_| {}).await;
        let mut stopping = checking_state();
        stopping.mode = Mode::Stopping;
        let mut done = checking_state();
        done.mode = Mode::Done;
        done.input_lost = true;
        done.fastest = vec![fast("103.152.112.162:1080", 312)];

        for state in [&State::default(), &checking_state(), &stopping, &done] {
            for width in (1..140).step_by(3) {
                for height in 1..40 {
                    render(&config, state, width, height);
                }
            }
        }
    }
}
