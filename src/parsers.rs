use std::sync::LazyLock;

use ipnetwork::IpNetwork;

pub static PROXY_REGEX: LazyLock<fancy_regex::Regex> = LazyLock::new(|| {
    let pattern = r"(?:^|[^0-9A-Za-z])(?:(?P<protocol>https?|socks[45]):\/\/)?(?:(?P<username>[0-9A-Za-z]{1,64}):(?P<password>[0-9A-Za-z]{1,64})@)?(?P<host>[A-Za-z][\-\.A-Za-z]{0,251}[A-Za-z]|[A-Za-z]|(?:[0-9]|[1-9][0-9]|1[0-9]{2}|2[0-4][0-9]|25[0-5])(?:\.(?:[0-9]|[1-9][0-9]|1[0-9]{2}|2[0-4][0-9]|25[0-5])){3}):(?P<port>[0-9]|[1-9][0-9]{1,3}|[1-5][0-9]{4}|6[0-4][0-9]{3}|65[0-4][0-9]{2}|655[0-2][0-9]|6553[0-5])(?=[^0-9A-Za-z]|$)";
    fancy_regex::RegexBuilder::new(pattern)
        .backtrack_limit(usize::MAX)
        .build()
        .unwrap()
});

static IPV4_REGEX: LazyLock<fancy_regex::Regex> = LazyLock::new(|| {
    let pattern = r"^\s*(?P<host>(?:[0-9]|[1-9][0-9]|1[0-9]{2}|2[0-4][0-9]|25[0-5])(?:\.(?:[0-9]|[1-9][0-9]|1[0-9]{2}|2[0-4][0-9]|25[0-5])){3})(?::(?:[0-9]|[1-9][0-9]{1,3}|[1-5][0-9]{4}|6[0-4][0-9]{3}|65[0-4][0-9]{2}|655[0-2][0-9]|6553[0-5]))?\s*$";
    fancy_regex::Regex::new(pattern).unwrap()
});

static CIDR_REGEX: LazyLock<fancy_regex::Regex> = LazyLock::new(|| {
    let pattern = r"^\s*(?P<network>(?:[0-9]|[1-9][0-9]|1[0-9]{2}|2[0-4][0-9]|25[0-5])(?:\.(?:[0-9]|[1-9][0-9]|1[0-9]{2}|2[0-4][0-9]|25[0-5])){3})/(?P<prefix>[0-9]|[12][0-9]|3[0-2]):(?P<port>[0-9]|[1-9][0-9]{1,3}|[1-5][0-9]{4}|6[0-4][0-9]{3}|65[0-4][0-9]{2}|655[0-2][0-9]|6553[0-5])\s*$";
    fancy_regex::Regex::new(pattern).unwrap()
});

pub fn parse_ipv4(s: &str) -> Option<String> {
    if let Ok(Some(captures)) = IPV4_REGEX.captures(s) {
        captures.name("host").map(|capture| capture.as_str().to_owned())
    } else {
        None
    }
}

/// Expands CIDR ranges in text into individual IP:port entries
/// Supports format like "192.168.1.0/24:8080" which expands to all IPs in the range
pub fn expand_cidr_ranges(text: &str) -> String {
    let mut result = String::new();
    
    for line in text.lines() {
        let line = line.trim();
        if let Ok(Some(captures)) = CIDR_REGEX.captures(line) {
            // Extract CIDR range and port
            if let (Some(network), Some(port)) = (captures.name("network"), captures.name("port")) {
                let cidr_str = format!("{}/{}", 
                    network.as_str(), 
                    captures.name("prefix").unwrap().as_str()
                );
                
                match cidr_str.parse::<IpNetwork>() {
                    Ok(network) => {
                        // Expand the network to individual IPs
                        for ip in network.iter() {
                            if ip.is_ipv4() {
                                result.push_str(&format!("{}:{}\n", ip, port.as_str()));
                            }
                        }
                    }
                    Err(_) => {
                        // If parsing fails, keep the original line
                        result.push_str(line);
                        result.push('\n');
                    }
                }
            } else {
                // If regex matches but capture groups are missing, keep the original line
                result.push_str(line);
                result.push('\n');
            }
        } else {
            // Not a CIDR range, keep the original line
            result.push_str(line);
            result.push('\n');
        }
    }
    
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cidr_expansion() {
        // Test basic CIDR expansion
        let input = "192.168.1.0/30:8080";
        let result = expand_cidr_ranges(input);
        let lines: Vec<&str> = result.trim().split('\n').collect();
        
        assert_eq!(lines.len(), 4);
        assert!(lines.contains(&"192.168.1.0:8080"));
        assert!(lines.contains(&"192.168.1.1:8080"));
        assert!(lines.contains(&"192.168.1.2:8080"));
        assert!(lines.contains(&"192.168.1.3:8080"));
    }

    #[test]
    fn test_mixed_input() {
        let input = "192.168.1.0/31:8080\n127.0.0.1:9090\ninvalid-line";
        let result = expand_cidr_ranges(input);
        let lines: Vec<&str> = result.trim().split('\n').collect();
        
        // Should have 2 CIDR-expanded IPs + 1 regular IP + 1 invalid line
        assert_eq!(lines.len(), 4);
        assert!(lines.contains(&"192.168.1.0:8080"));
        assert!(lines.contains(&"192.168.1.1:8080"));
        assert!(lines.contains(&"127.0.0.1:9090"));
        assert!(lines.contains(&"invalid-line"));
    }

    #[test]
    fn test_single_ip_cidr() {
        let input = "10.0.0.1/32:3128";
        let result = expand_cidr_ranges(input);
        assert_eq!(result.trim(), "10.0.0.1:3128");
    }
}
