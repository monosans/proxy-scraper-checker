use crate::proxy::ProxyType;

#[allow(dead_code)]
pub(crate) enum AppEvent {
    GeoDbTotal(Option<u64>),
    GeoDbDownloaded(usize),

    SourcesTotal(ProxyType, usize),
    SourceScraped(ProxyType),

    TotalProxies(ProxyType, usize),
    ProxyChecked(ProxyType),
    ProxyWorking(ProxyType),

    Done,
}

pub(crate) enum Event {
    #[cfg(feature = "tui")]
    Tick,
    #[cfg(feature = "tui")]
    Crossterm(crossterm::event::Event),
    #[allow(dead_code)]
    App(AppEvent),
}
