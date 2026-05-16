//! Robots.txt parser (RFC 9309).

use std::collections::HashMap;

/// Stores robots.txt rules per domain.
#[derive(Debug, Default)]
pub struct RobotStore {
    rules: HashMap<String, RobotRules>,
    #[allow(dead_code)]
    sitemaps: HashMap<String, Vec<String>>,
}

/// Per-domain rules with per-agent allow/disallow lists.
#[derive(Debug, Default)]
struct RobotRules {
    /// Per user-agent allow/disallow lists.
    agent_rules: HashMap<String, AgentRules>,
}

#[derive(Debug, Default)]
struct AgentRules {
    allow: Vec<String>,
    disallow: Vec<String>,
}

impl RobotStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add robots.txt content for a domain.
    pub fn add(&mut self, domain: &str, content: &str) {
        let domain = domain.to_lowercase();
        let mut rules = RobotRules::default();
        let mut current_agents: Vec<String> = vec!["*".to_string()];

        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some((directive, value)) = line.split_once(':') {
                let directive = directive.trim().to_lowercase();
                let value = value.trim();

                match directive.as_str() {
                    "user-agent" => {
                        current_agents.clear();
                        current_agents.push(value.to_lowercase());
                    }
                    "allow" => {
                        for agent in &current_agents {
                            let r = rules.agent_rules.entry(agent.clone()).or_default();
                            r.allow.push(value.to_string());
                        }
                    }
                    "disallow" => {
                        for agent in &current_agents {
                            let r = rules.agent_rules.entry(agent.clone()).or_default();
                            r.disallow.push(value.to_string());
                        }
                    }
                    _ => {}
                }
            }
        }

        self.rules.insert(domain, rules);
    }

    /// Check if a URL is allowed for a user-agent.
    pub fn is_allowed(&self, url: &str, user_agent: &str) -> bool {
        let path = extract_path(url);
        let domain = extract_domain(url);

        let rules = match self.rules.get(&domain) {
            Some(r) => r,
            None => return true, // No rules for this domain = allowed
        };

        // Collect the best-matching rule (longest pattern wins per RFC 9309)
        let mut best_match_allow = (0usize, true); // (pattern_len, is_allowed)
        let mut best_match_disallow = (0usize, false);

        // Check wildcard agent rules first
        if let Some(agent_rules) = rules.agent_rules.get("*") {
            for rule in &agent_rules.allow {
                if path_matches(rule, &path) {
                    let len = rule.len();
                    if len > best_match_allow.0 {
                        best_match_allow = (len, true);
                    }
                }
            }
            for rule in &agent_rules.disallow {
                if path_matches(rule, &path) {
                    let len = rule.len();
                    if len > best_match_disallow.0 {
                        best_match_disallow = (len, false);
                    }
                }
            }
        }

        // Check specific agent rules (override wildcard)
        let ua_lower = user_agent.to_lowercase();
        if let Some(agent_rules) = rules.agent_rules.get(&ua_lower) {
            for rule in &agent_rules.allow {
                if path_matches(rule, &path) {
                    let len = rule.len();
                    if len > best_match_allow.0 {
                        best_match_allow = (len, true);
                    }
                }
            }
            for rule in &agent_rules.disallow {
                if path_matches(rule, &path) {
                    let len = rule.len();
                    if len > best_match_disallow.0 {
                        best_match_disallow = (len, false);
                    }
                }
            }
        }

        // RFC 9309 §2.3.2: longest pattern wins; allow wins ties
        best_match_allow.0 >= best_match_disallow.0
    }
}

fn extract_domain(url: &str) -> String {
    if let Some(start) = url.find("://") {
        let after = &url[start + 3..];
        let host_end = after.find('/').unwrap_or(after.len());
        let host = &after[..host_end];
        // Strip port if present
        let host = host.split(':').next().unwrap_or(host);
        host.to_lowercase()
    } else {
        // Might be a relative URL or just a path — no domain info
        String::new()
    }
}

fn extract_path(url: &str) -> String {
    if let Some(start) = url.find("://") {
        let after = &url[start + 3..];
        if let Some(pos) = after.find('/') {
            after[pos..].to_string()
        } else {
            "/".to_string()
        }
    } else {
        url.to_string()
    }
}

fn path_matches(pattern: &str, path: &str) -> bool {
    if pattern == "/" {
        return true;
    }
    if pattern.is_empty() {
        return false;
    }
    if let Some(p) = pattern.strip_suffix('$') {
        return path == p;
    }
    if let Some(p) = pattern.strip_suffix('*') {
        return path.starts_with(p);
    }
    path.starts_with(pattern)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_robots() {
        let mut store = RobotStore::new();
        store.add(
            "example.com",
            "User-agent: *\nDisallow: /admin/\nAllow: /public/\n",
        );

        assert!(!store.is_allowed("https://example.com/admin/secret", "MyBot"));
        assert!(store.is_allowed("https://example.com/public/page", "MyBot"));
        assert!(store.is_allowed("https://example.com/other", "MyBot"));
    }

    #[test]
    fn test_is_allowed_allowed_path() {
        let mut store = RobotStore::new();
        store.add(
            "example.com",
            "User-agent: *\nAllow: /public/\nDisallow: /private/\n",
        );

        assert!(
            store.is_allowed("https://example.com/public/page", "Bot"),
            "/public/ should be allowed"
        );
        assert!(
            store.is_allowed("https://example.com/anything", "Bot"),
            "unlisted path should be allowed"
        );
    }

    #[test]
    fn test_is_allowed_disallowed_path() {
        let mut store = RobotStore::new();
        store.add("example.com", "User-agent: *\nDisallow: /private/\n");

        assert!(
            !store.is_allowed("https://example.com/private/secret", "Bot"),
            "/private/ should be disallowed"
        );
    }

    #[test]
    fn test_per_agent_rules_isolation() {
        let mut store = RobotStore::new();
        store.add(
            "example.com",
            "User-agent: *\nDisallow: /\n\nUser-agent: Googlebot\nAllow: /\n",
        );

        // Wildcard agent: disallowed
        assert!(!store.is_allowed("https://example.com/page", "RandomBot"));
        // Googlebot: allowed
        assert!(store.is_allowed("https://example.com/page", "Googlebot"));
    }

    #[test]
    fn test_allow_overrides_disallow_longest_wins() {
        let mut store = RobotStore::new();
        store.add(
            "example.com",
            "User-agent: *\nDisallow: /admin/\nAllow: /admin/public/\n",
        );

        assert!(
            store.is_allowed("https://example.com/admin/public/file", "Bot"),
            "/admin/public/ should be allowed (longer Allow pattern)"
        );
        assert!(
            !store.is_allowed("https://example.com/admin/other", "Bot"),
            "/admin/other should be disallowed"
        );
    }

    #[test]
    fn test_wildcard_matching() {
        let mut store = RobotStore::new();
        store.add(
            "example.com",
            "User-agent: *\nDisallow: /secret*\nDisallow: /files/*\n",
        );

        assert!(!store.is_allowed("https://example.com/secret_stuff", "Bot"));
        assert!(!store.is_allowed("https://example.com/files/", "Bot"));
        assert!(store.is_allowed("https://example.com/public", "Bot"));
    }

    #[test]
    fn test_no_rules_means_allowed() {
        let store = RobotStore::new();
        assert!(
            store.is_allowed("https://unknown.com/page", "Bot"),
            "no rules for domain should mean allowed"
        );
    }

    #[test]
    fn test_comments_and_blank_lines_ignored() {
        let mut store = RobotStore::new();
        store.add(
            "example.com",
            "# This is a comment\n\nUser-agent: *\n# Another comment\nDisallow: /admin/\n\n",
        );

        assert!(!store.is_allowed("https://example.com/admin/page", "Bot"));
        assert!(store.is_allowed("https://example.com/public", "Bot"));
    }
}
