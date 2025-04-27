#[cfg(feature = "tui")]
use crate::proxy::ProxyType;

pub enum AppEvent {
    #[cfg(feature = "tui")]
    GeoDbTotal(Option<u64>),
    #[cfg(feature = "tui")]
    GeoDbDownloaded(usize),

    #[cfg(feature = "tui")]
    SourcesTotal(ProxyType, usize),
    #[cfg(feature = "tui")]
    SourceScraped(ProxyType),

    #[cfg(feature = "tui")]
    TotalProxies(ProxyType, usize),
    #[cfg(feature = "tui")]
    ProxyChecked(ProxyType),
    #[cfg(feature = "tui")]
    ProxyWorking(ProxyType),

    Done,
}

pub enum Event {
    #[cfg(feature = "tui")]
    Tick,
    #[cfg(feature = "tui")]
    Crossterm(crossterm::event::Event),
    App(AppEvent),
}
