use std::time::Duration;

use compact_str::CompactString;

use crate::{ipdb, proxy::ProxyType};

#[derive(Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum Stage {
    #[default]
    Scraping,
    Checking,
    Saving,
}

pub enum AppEvent {
    Stage(Stage),

    IpDbTotal(ipdb::DbType, Option<u64>),
    IpDbDownloaded(ipdb::DbType, usize),
    IpDbFinished(ipdb::DbType, bool),

    SourcesTotal(ProxyType, usize),
    SourceScraped(ProxyType),

    TotalProxies(ProxyType, usize),
    ProxyChecked(ProxyType),
    ProxyWorking {
        protocol: ProxyType,
        address: CompactString,
        latency: Duration,
    },

    Saved(usize),
    InputLost,
    Done,
    Quit,
}

pub enum Event {
    Tick,
    Crossterm(crossterm::event::Event),
    App(AppEvent),
}
