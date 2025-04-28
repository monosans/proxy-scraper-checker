#[cfg(feature = "tui")]
use crate::proxy::ProxyType;

#[cfg(feature = "tui")]
pub enum AppEvent {
    GeoDbTotal(Option<u64>),
    GeoDbDownloaded(usize),

    SourcesTotal(ProxyType, usize),
    SourceScraped(ProxyType),

    TotalProxies(ProxyType, usize),
    ProxyChecked(ProxyType),
    ProxyWorking(ProxyType),

    Done,
}

#[cfg(feature = "tui")]
pub enum Event {
    Tick,
    Crossterm(crossterm::event::Event),
    App(AppEvent),
}

#[cfg(not(feature = "tui"))]
pub enum Event {}
