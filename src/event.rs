use crate::{ipdb, proxy::ProxyType};

pub enum AppEvent {
    IpDbTotal(ipdb::DbType, Option<u64>),
    IpDbDownloaded(ipdb::DbType, usize),

    SourcesTotal(ProxyType, usize),
    SourceScraped(ProxyType),

    TotalProxies(ProxyType, usize),
    ProxyChecked(ProxyType),
    ProxyWorking(ProxyType),

    Done,
    Quit,
}

pub enum Event {
    Tick,
    Crossterm(crossterm::event::Event),
    App(AppEvent),
}
