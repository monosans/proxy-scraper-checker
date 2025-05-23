#[cfg(feature = "tui")]
use crate::ipdb;
#[cfg(feature = "tui")]
use crate::proxy::ProxyType;

#[cfg(feature = "tui")]
pub enum AppEvent {
    IpDbTotal(ipdb::DbType, Option<u64>),
    IpDbDownloaded(ipdb::DbType, usize),

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
