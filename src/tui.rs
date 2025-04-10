use color_eyre::eyre::WrapErr;
use crossterm::event::{
    Event as CrosstermEvent, KeyCode, KeyModifiers, MouseEventKind,
};
use futures::{FutureExt, StreamExt};
use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Direction, Flex, Layout},
    style::{Color, Style},
    text::{Line, Text},
    widgets::{Block, Gauge, Row, Table},
};
use tui_logger::{TuiLoggerWidget, TuiWidgetEvent, TuiWidgetState};

use crate::{
    event::{AppEvent, AppMode, AppState, Event},
    proxy::ProxyType,
};

const FPS: f64 = 30.0;

async fn tick_event_listener(
    tx: tokio::sync::mpsc::UnboundedSender<Event>,
) -> color_eyre::Result<()> {
    let mut tick =
        tokio::time::interval(tokio::time::Duration::from_secs_f64(1.0 / FPS));
    loop {
        tokio::select! {
            biased;
            () = tx.closed() => {
                break Ok(())
            },
            _ = tick.tick() =>{
                tx.send(Event::Tick)?;
            }
        }
    }
}

async fn crossterm_event_listener(
    tx: tokio::sync::mpsc::UnboundedSender<Event>,
) -> color_eyre::Result<()> {
    let mut reader = crossterm::event::EventStream::new();
    loop {
        tokio::select! {
            biased;
            () = tx.closed() => {
                break Ok(())
            },
            Some(Ok(event)) = reader.next().fuse() => {
                tx.send(Event::Crossterm(event))?;
            }
        }
    }
}

pub(crate) async fn run(
    mut terminal: ratatui::DefaultTerminal,
    tx: tokio::sync::mpsc::UnboundedSender<Event>,
    mut rx: tokio::sync::mpsc::UnboundedReceiver<Event>,
) -> color_eyre::Result<()> {
    let tick_task = tokio::spawn(tick_event_listener(tx.clone()));
    let crossterm_task = tokio::spawn(crossterm_event_listener(tx));
    let mut app_state = AppState::new();
    let logger_state = TuiWidgetState::default();
    while !matches!(app_state.mode, AppMode::Quit) {
        if let Some(event) = rx.recv().await {
            if handle_event(event, &mut app_state, &logger_state) {
                terminal
                    .draw(|frame| draw(frame, &app_state, &logger_state))
                    .wrap_err("failed to draw tui")?;
            }
        }
    }
    drop(rx);
    tick_task.await.wrap_err("failed to spawn tui tick task")??;
    crossterm_task.await.wrap_err("failed to spawn tui crossterm task")??;
    Ok(())
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
            Constraint::Fill(1),
            Constraint::Length((state.sources_total.len() * 3 + 2) as u16),
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

    let rows = vec!["Protocol", "Working", "Working %"];
    let bottom_layout = Layout::default()
        .direction(Direction::Horizontal)
        .constraints(vec![
            Constraint::Fill(1),
            Constraint::Fill(1),
            Constraint::Length(
                (rows.iter().map(|row| row.len()).sum::<usize>()
                    + ((rows.len() - 1) * 3)
                    + 2) as u16,
            ),
        ])
        .split(outer_layout[1]);

    let mut lines = Vec::new();
    if let AppMode::Done = state.mode {
        lines.push(
            Line::from("Enter/ESC/q/Ctrl-C - exit")
                .style(Style::default().fg(Color::Red)),
        );
    }
    lines.push(Line::from("Up/PageUp/k - scroll logs up"));
    lines.push(Line::from("Down/PageDown/j - scroll logs down"));
    f.render_widget(Text::from(lines).centered(), outer_layout[2]);

    let scrape_area = bottom_layout[0];
    let scrape_block = Block::bordered().title("Scraping");
    f.render_widget(scrape_block.clone(), scrape_area);
    let scrape_inner = scrape_block.inner(scrape_area);
    let mut proxy_types: Vec<ProxyType> =
        state.sources_total.keys().cloned().collect();
    proxy_types.sort();
    let scrape_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints(
            proxy_types.iter().map(|_| Constraint::Fill(1)).collect::<Vec<_>>(),
        )
        .split(scrape_inner);
    for (i, proxy_type) in proxy_types.iter().enumerate() {
        let total = state.sources_total.get(proxy_type).copied().unwrap_or(0);
        let scraped =
            state.sources_scraped.get(proxy_type).copied().unwrap_or(0);
        f.render_widget(
            Gauge::default()
                .ratio(if total == 0 {
                    0.0
                } else {
                    scraped as f64 / total as f64
                })
                .block(
                    Block::bordered()
                        .title(proxy_type.to_string().to_uppercase()),
                )
                .label(format!("{scraped}/{total}")),
            scrape_layout[i],
        );
    }

    let check_area = bottom_layout[1];
    let check_block = Block::bordered().title("Checking");
    f.render_widget(check_block.clone(), check_area);
    let check_inner = check_block.inner(check_area);
    let check_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints(
            proxy_types.iter().map(|_| Constraint::Fill(1)).collect::<Vec<_>>(),
        )
        .split(check_inner);
    for (i, proxy_type) in proxy_types.iter().enumerate() {
        let total = state.proxies_total.get(proxy_type).copied().unwrap_or(0);
        let checked =
            state.proxies_checked.get(proxy_type).copied().unwrap_or(0);
        let gauge = Gauge::default()
            .block(
                Block::bordered().title(proxy_type.to_string().to_uppercase()),
            )
            .label(format!("{checked}/{total}"))
            .ratio(if total == 0 {
                0.0
            } else {
                checked as f64 / total as f64
            });
        f.render_widget(gauge, check_layout[i]);
    }

    let result_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints(vec![Constraint::Length(3), Constraint::Min(0)])
        .split(bottom_layout[2]);
    f.render_widget(
        Gauge::default()
            .block(Block::bordered().title("GeoDB download"))
            .ratio(if state.geodb_total == 0 {
                1.0
            } else {
                (state.geodb_downloaded as f64) / (state.geodb_total as f64)
            }),
        result_layout[0],
    );
    let table = Table::new(
        proxy_types
            .iter()
            .enumerate()
            .map(move |(i, proxy_type)| {
                let working = state
                    .proxies_working
                    .get(proxy_type)
                    .copied()
                    .unwrap_or_default();
                let total = state
                    .proxies_total
                    .get(proxy_type)
                    .copied()
                    .unwrap_or_default();
                let percentage = if total != 0 {
                    (working as f64 / total as f64) * 100.0
                } else {
                    0.0
                };
                Row::new(vec![
                    proxy_type.to_string().to_uppercase(),
                    working.to_string(),
                    format!("{percentage:.1}%"),
                ])
                .top_margin((i != 0).into())
            })
            .collect::<Vec<_>>(),
        rows.iter()
            .map(|row| Constraint::Length(row.len() as u16))
            .collect::<Vec<_>>(),
    )
    .flex(Flex::SpaceBetween)
    .header(Row::new(rows))
    .block(Block::bordered().title("Result"));
    f.render_widget(table, result_layout[1]);
}

fn handle_event(
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
                    state.mode = AppMode::Done;
                }
            }
            false
        }
    }
}
