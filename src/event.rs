use crate::{ipdb, proxy::ProxyType};

#[derive(Clone, Copy)]
pub enum AppEvent {
    IpDbTotal(ipdb::DbType, Option<u64>),
    IpDbDownloaded(ipdb::DbType, usize),
    IpDbFinished(ipdb::DbType, bool),

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
