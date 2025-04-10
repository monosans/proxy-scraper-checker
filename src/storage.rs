use std::collections::{HashMap, HashSet};

use crate::proxy::{Proxy, ProxyType};

pub(crate) struct ProxyStorage {
    proxies: HashSet<Proxy>,
    enabled_protocols: HashSet<ProxyType>,
}

impl ProxyStorage {
    pub(crate) fn new(protocols: HashSet<ProxyType>) -> Self {
        Self { proxies: HashSet::new(), enabled_protocols: protocols }
    }

    pub(crate) fn insert(&mut self, proxy: Proxy) {
        if self.enabled_protocols.contains(&proxy.protocol) {
            self.proxies.insert(proxy);
        }
    }

    pub(crate) fn get_grouped(&self) -> HashMap<ProxyType, Vec<&Proxy>> {
        let mut groups: HashMap<ProxyType, Vec<&Proxy>> = HashMap::new();
        for proxy in &self.proxies {
            groups.entry(proxy.protocol.clone()).or_default().push(proxy);
        }
        for protocol in &self.enabled_protocols {
            groups.entry(protocol.clone()).or_default();
        }
        groups
    }

    pub(crate) fn iter(&self) -> std::collections::hash_set::Iter<'_, Proxy> {
        self.proxies.iter()
    }
}

impl IntoIterator for ProxyStorage {
    type IntoIter = std::collections::hash_set::IntoIter<Proxy>;
    type Item = Proxy;

    fn into_iter(self) -> Self::IntoIter {
        self.proxies.into_iter()
    }
}
