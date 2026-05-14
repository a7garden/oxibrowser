//! Robots.txt parser (RFC 9309).

use std::collections::HashMap;

/// Stores robots.txt rules per domain.
#[derive(Debug, Default)]
pub struct RobotStore {
    rules: HashMap<String, RobotRules>,
    #[allow(dead_code)]
    sitemaps: HashMap<String, Vec<String>>,
}

#[derive(Debug, Default)]
struct RobotRules {
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
                            let r = rules_for_agent(&mut rules, agent);
                            r.allow.push(value.to_string());
                        }
                    }
                    "disallow" => {
                        for agent in &current_agents {
                            let r = rules_for_agent(&mut rules, agent);
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
        let ua_lower = user_agent.to_lowercase();

        // Find matching rules
        for agent in [&ua_lower, &String::from("*")] {
            if let Some(rules) = self.rules.get(agent) {
                if !rules.disallow.is_empty() || !rules.allow.is_empty() {
                    // Disallow takes priority
                    for rule in &rules.disallow {
                        if path_matches(rule, &path) {
                            return false;
                        }
                    }
                    return true;
                }
            }
        }

        true // No rules = allowed
    }
}

fn rules_for_agent<'a>(rules: &'a mut RobotRules, _agent: &str) -> &'a mut RobotRules {
    // For simplicity, we use a single shared ruleset per domain
    // Full RFC 9309 would need per-agent rules
    rules
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
