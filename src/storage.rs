use std::collections::{HashMap, HashSet, hash_set};

use crate::proxy::{Proxy, ProxyType};

pub struct ProxyStorage {
    proxies: HashSet<Proxy>,
    enabled_protocols: HashSet<ProxyType>,
}

impl ProxyStorage {
    pub fn new(protocols: HashSet<ProxyType>) -> Self {
        Self { proxies: HashSet::new(), enabled_protocols: protocols }
    }

    pub fn insert(&mut self, proxy: Proxy) {
        if self.enabled_protocols.contains(&proxy.protocol) {
            self.proxies.insert(proxy);
        }
    }

    pub fn get_grouped(&self) -> HashMap<ProxyType, Vec<&Proxy>> {
        let mut groups: HashMap<_, _> = self
            .enabled_protocols
            .iter()
            .map(move |p| (p.clone(), Vec::new()))
            .collect();
        for proxy in &self.proxies {
            if let Some(proxies) = groups.get_mut(&proxy.protocol) {
                proxies.push(proxy);
            }
        }
        groups
    }

    pub fn iter(&self) -> hash_set::Iter<'_, Proxy> {
        self.proxies.iter()
    }
}

impl IntoIterator for ProxyStorage {
    type IntoIter = hash_set::IntoIter<Proxy>;
    type Item = Proxy;

    fn into_iter(self) -> Self::IntoIter {
        self.proxies.into_iter()
    }
}
