use crate::proxy::ProxyType;

#[cfg_attr(not(feature = "tui"), expect(dead_code))]
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

pub enum Event {
    #[cfg(feature = "tui")]
    Tick,
    #[cfg(feature = "tui")]
    Crossterm(crossterm::event::Event),
    App(AppEvent),
}
