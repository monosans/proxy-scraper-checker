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
    let pattern = r"(?:^|[^0-9A-Za-z])(?P<network>(?:[0-9]|[1-9][0-9]|1[0-9]{2}|2[0-4][0-9]|25[0-5])(?:\.(?:[0-9]|[1-9][0-9]|1[0-9]{2}|2[0-4][0-9]|25[0-5])){3})/(?P<prefix>[0-9]|[12][0-9]|3[0-2]):(?P<port>[0-9]|[1-9][0-9]{1,3}|[1-5][0-9]{4}|6[0-4][0-9]{3}|65[0-4][0-9]{2}|655[0-2][0-9]|6553[0-5])(?=[^0-9A-Za-z]|$)";
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
/// Handles various separators (spaces, commas, newlines, etc.) between entries
pub fn expand_cidr_ranges(text: &str) -> String {
    let mut result = text.to_string();
    let mut offset: i32 = 0;
    
    // Find all CIDR matches and expand them
    let captures: Vec<_> = CIDR_REGEX.captures_iter(text)
        .filter_map(|m| m.ok())
        .collect();
    
    for capture in captures {
        if let (Some(network), Some(prefix), Some(port)) = (
            capture.name("network"), 
            capture.name("prefix"), 
            capture.name("port")
        ) {
            let cidr_str = format!("{}/{}", network.as_str(), prefix.as_str());
            
            match cidr_str.parse::<IpNetwork>() {
                Ok(network) => {
                    // Generate expanded IPs
                    let expanded_ips: Vec<String> = network.iter()
                        .filter(|ip| ip.is_ipv4())
                        .map(|ip| format!("{}:{}", ip, port.as_str()))
                        .collect();
                    
                    if !expanded_ips.is_empty() {
                        // Get the full match including any leading non-alphanumeric character
                        let full_match = capture.get(0).unwrap();
                        let match_start = (full_match.start() as i32 + offset) as usize;
                        let match_end = (full_match.end() as i32 + offset) as usize;
                        
                        // Determine what separator to use by checking what follows
                        let separator = if match_end < result.len() {
                            let next_char = result.chars().nth(match_end);
                            match next_char {
                                Some('\n') => "\n",
                                Some('\t') => "\t", 
                                Some(',') => ",",
                                _ => " ",
                            }
                        } else {
                            "\n"
                        };
                        
                        // Join expanded IPs with the detected separator
                        let replacement = expanded_ips.join(separator);
                        
                        // Handle case where match starts with a delimiter character
                        let (_actual_start, prefix_char) = if match_start > 0 {
                            let prev_char = result.chars().nth(match_start);
                            if prev_char.map_or(false, |c| !c.is_ascii_alphanumeric()) {
                                (match_start + 1, result.chars().nth(match_start).unwrap().to_string())
                            } else {
                                (match_start, String::new())
                            }
                        } else {
                            (match_start, String::new())
                        };
                        
                        let final_replacement = format!("{}{}", prefix_char, replacement);
                        
                        // Replace the CIDR pattern with expanded IPs
                        result.replace_range(match_start..match_end, &final_replacement);
                        
                        // Update offset for subsequent replacements
                        let len_diff = final_replacement.len() as i32 - (match_end - match_start) as i32;
                        offset += len_diff;
                    }
                }
                Err(_) => {
                    // If parsing fails, leave the original text unchanged
                    continue;
                }
            }
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

    #[test]
    fn test_non_newline_separated_behavior() {
        // Test space-separated entries with CIDR expansion
        let input = "192.168.1.0/31:8080 127.0.0.1:9090";
        let result = expand_cidr_ranges(input);
        
        // Should expand the CIDR range and preserve the regular proxy
        assert!(result.contains("192.168.1.0:8080"));
        assert!(result.contains("192.168.1.1:8080"));
        assert!(result.contains("127.0.0.1:9090"));
    }

    #[test] 
    fn test_multiple_cidr_same_line_behavior() {
        // Test multiple CIDR ranges on same line
        let input = "192.168.1.0/31:8080 10.0.0.0/31:3128";
        let result = expand_cidr_ranges(input);
        
        // Should expand both CIDR ranges
        assert!(result.contains("192.168.1.0:8080"));
        assert!(result.contains("192.168.1.1:8080"));
        assert!(result.contains("10.0.0.0:3128"));
        assert!(result.contains("10.0.0.1:3128"));
    }

    #[test]
    fn test_comma_separated_cidr() {
        let input = "192.168.1.0/31:8080,10.0.0.0/31:3128";
        let result = expand_cidr_ranges(input);
        
        // Should expand both CIDR ranges and preserve comma separation
        assert!(result.contains("192.168.1.0:8080"));
        assert!(result.contains("192.168.1.1:8080"));
        assert!(result.contains("10.0.0.0:3128"));
        assert!(result.contains("10.0.0.1:3128"));
    }

    #[test]
    fn test_mixed_separators() {
        let input = "192.168.1.0/31:8080\t10.0.0.1:3128,203.0.113.0/31:1080 127.0.0.1:9090";
        let result = expand_cidr_ranges(input);
        
        // Should expand CIDR ranges and preserve non-CIDR entries
        assert!(result.contains("192.168.1.0:8080"));
        assert!(result.contains("192.168.1.1:8080"));
        assert!(result.contains("10.0.0.1:3128"));
        assert!(result.contains("203.0.113.0:1080"));
        assert!(result.contains("203.0.113.1:1080"));
        assert!(result.contains("127.0.0.1:9090"));
    }
}
