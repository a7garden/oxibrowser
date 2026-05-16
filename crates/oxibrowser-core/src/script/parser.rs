//! YAML parser for browser scripts.
//!
//! Parses a YAML string into a `ScriptConfig` (top-level config + Vec<Step>`).
//!
//! # YAML Schema
//!
//! ```yaml
//! name: <string>
//! timeout: <ms>              # default: 30000
//!
//! on_error:
//!   action: abort | continue  # default: abort
//!   screenshot: <bool>        # default: false
//!   retry:
//!     count: <n>
//!     delay_ms: <ms>
//!
//! steps:
//!   - step_type: <step_type>
//!     data: <step_data>
//! ```
//!
//! See `types.rs` for the full list of step types and their YAML representation.

use super::types::ScriptConfig;

use std::path::Path;

/// Parse a YAML string into a `ScriptConfig`.
pub fn parse_script(yaml: &str) -> Result<ScriptConfig, ScriptError> {
    serde_yaml::from_str(yaml).map_err(|e| ScriptError::Parse(e.to_string()))
}

/// Parse a YAML file into a `ScriptConfig`.
pub fn parse_script_file(path: &Path) -> Result<ScriptConfig, ScriptError> {
    let yaml = std::fs::read_to_string(path).map_err(|e| ScriptError::Io(e.to_string()))?;
    parse_script(&yaml)
}

/// Parse a YAML file or string into a `ScriptConfig` (auto-detect).
pub fn parse_script_from(path_or_yaml: &str) -> Result<ScriptConfig, ScriptError> {
    // Try as file path first
    let path = Path::new(path_or_yaml);
    if path.exists() {
        parse_script_file(path)
    } else {
        // Treat as raw YAML string
        parse_script(path_or_yaml)
    }
}

/// A parsing error for scripts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScriptError {
    /// YAML parsing failed.
    Parse(String),
    /// File I/O error.
    Io(String),
    /// Script execution error (step failed, etc.).
    Exec(String),
}

impl std::fmt::Display for ScriptError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ScriptError::Parse(msg) => write!(f, "script parse error: {msg}"),
            ScriptError::Io(msg) => write!(f, "script I/O error: {msg}"),
            ScriptError::Exec(msg) => write!(f, "script exec error: {msg}"),
        }
    }
}

impl std::error::Error for ScriptError {}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::types::{ErrorAction, Step};

    #[test]
    fn test_parse_minimal_script() {
        let yaml = "steps:\n  - step_type: goto\n    data:\n      goto: \"https://example.com\"\n  - step_type: click\n    data:\n      click: \"button#submit\"";
        let config = parse_script(yaml).unwrap();
        assert_eq!(config.steps.len(), 2);
        assert_eq!(config.steps[0].name(), "goto");
        assert_eq!(config.steps[1].name(), "click");
    }

    #[test]
    fn test_parse_with_name_and_timeout() {
        let yaml = "name: \"Login Test\"\ntimeout: 60000\nsteps:\n  - step_type: goto\n    data:\n      goto: \"https://example.com\"";
        let config = parse_script(yaml).unwrap();
        assert_eq!(config.name.as_deref(), Some("Login Test"));
        assert_eq!(config.timeout, 60000);
    }

    #[test]
    fn test_parse_back_forward_reload() {
        let yaml = "steps:\n  - step_type: back\n  - step_type: forward\n  - step_type: reload";
        let config = parse_script(yaml).unwrap();
        assert_eq!(config.steps[0].name(), "back");
        assert_eq!(config.steps[1].name(), "forward");
        assert_eq!(config.steps[2].name(), "reload");
    }

    #[test]
    fn test_parse_goto_with_wait() {
        let yaml = "steps:\n  - step_type: goto\n    data:\n      goto: \"https://example.com\"\n      wait: \".content-loaded\"";
        let config = parse_script(yaml).unwrap();
        match &config.steps[0] {
            Step::Goto { data } => {
                assert_eq!(data.goto, "https://example.com");
                assert_eq!(data.wait.as_deref(), Some(".content-loaded"));
            }
            _ => panic!("expected Goto step"),
        }
    }

    #[test]
    fn test_parse_post() {
        let yaml = "steps:\n  - step_type: post\n    data:\n      url: \"https://example.com/api\"\n      body: \"user=admin&pass=secret\"\n      content_type: \"application/x-www-form-urlencoded\"";
        let config = parse_script(yaml).unwrap();
        match &config.steps[0] {
            Step::Post { data } => {
                assert_eq!(data.url, "https://example.com/api");
                assert_eq!(data.body, "user=admin&pass=secret");
                assert_eq!(data.content_type, "application/x-www-form-urlencoded");
            }
            _ => panic!("expected Post step"),
        }
    }

    #[test]
    fn test_parse_fill_type_clear() {
        let yaml = "steps:\n  - step_type: fill\n    data:\n      selector: \"#username\"\n      value: \"admin\"\n  - step_type: type\n    data:\n      selector: \"#password\"\n      text: \"secret\"\n  - step_type: clear\n    data:\n      selector: \"#search\"";
        let config = parse_script(yaml).unwrap();
        assert_eq!(config.steps[0].name(), "fill");
        assert_eq!(config.steps[1].name(), "type");
        assert_eq!(config.steps[2].name(), "clear");
    }

    #[test]
    fn test_parse_check_uncheck_select() {
        let yaml = "steps:\n  - step_type: check\n    data:\n      selector: \"#agree\"\n  - step_type: uncheck\n    data:\n      selector: \"#newsletter\"\n  - step_type: select\n    data:\n      selector: \"#country\"\n      value: US";
        let config = parse_script(yaml).unwrap();
        assert_eq!(config.steps[0].name(), "check");
        assert_eq!(config.steps[1].name(), "uncheck");
        assert_eq!(config.steps[2].name(), "select");
    }

    #[test]
    fn test_parse_press_scroll_drag() {
        let yaml = "steps:\n  - step_type: press\n    data:\n      press: Enter\n  - step_type: scroll\n    data:\n      x: 0\n      y: 200\n  - step_type: drag\n    data:\n      from: \".handle\"\n      to: \".drop-zone\"";
        let config = parse_script(yaml).unwrap();
        assert_eq!(config.steps[0].name(), "press");
        assert_eq!(config.steps[1].name(), "scroll");
        assert_eq!(config.steps[2].name(), "drag");
    }

    #[test]
    fn test_parse_evaluate() {
        let yaml = "steps:\n  - step_type: evaluate\n    data:\n      evaluate: \"document.title\"\n  - step_type: evaluate\n    data:\n      evaluate: \"document.querySelector('.user').textContent\"\n      await: true\n      save: username";
        let config = parse_script(yaml).unwrap();
        assert_eq!(config.steps[0].name(), "evaluate");
        match &config.steps[1] {
            Step::Evaluate { data } => {
                assert!(data.r#await);
                assert_eq!(data.save.as_deref(), Some("username"));
            }
            _ => panic!("expected Evaluate step"),
        }
    }

    #[test]
    fn test_parse_wait() {
        let yaml = "steps:\n  - step_type: wait\n    data:\n      wait: \".modal\"\n      timeout: 5000";
        let config = parse_script(yaml).unwrap();
        match &config.steps[0] {
            Step::Wait { data } => {
                assert_eq!(data.wait, ".modal");
                assert_eq!(data.timeout, Some(5000));
            }
            _ => panic!("expected Wait step"),
        }
    }

    #[test]
    fn test_parse_extract() {
        let yaml = "steps:\n  - step_type: extract\n    data:\n      selector: \".price\"\n      all: true\n  - step_type: extract\n    data:\n      selector: \"a\"\n      links: true\n      save: links";
        let config = parse_script(yaml).unwrap();
        assert_eq!(config.steps[0].name(), "extract");
        assert_eq!(config.steps[1].name(), "extract");
        match &config.steps[1] {
            Step::Extract { data } => {
                assert!(data.links);
                assert_eq!(data.save.as_deref(), Some("links"));
            }
            _ => panic!("expected Extract step"),
        }
    }

    #[test]
    fn test_parse_content_screenshot_load_resources() {
        let yaml = "steps:\n  - step_type: content\n    data:\n      format: markdown\n  - step_type: screenshot\n    data:\n      file: \"./output.png\"\n      width: 1200\n  - step_type: load_resources";
        let config = parse_script(yaml).unwrap();
        assert_eq!(config.steps[0].name(), "content");
        assert_eq!(config.steps[1].name(), "screenshot");
        assert_eq!(config.steps[2].name(), "load_resources");
    }

    #[test]
    fn test_parse_set_echo_sleep() {
        let yaml = "steps:\n  - step_type: set\n    data:\n      base_url: \"https://example.com\"\n      timeout: 30000\n  - step_type: echo\n    data:\n      echo: \"Starting login flow\"\n  - step_type: sleep\n    data:\n      sleep: 1000";
        let config = parse_script(yaml).unwrap();
        assert_eq!(config.steps[0].name(), "set");
        assert_eq!(config.steps[1].name(), "echo");
        assert_eq!(config.steps[2].name(), "sleep");
    }

    #[test]
    fn test_parse_if() {
        let yaml = "steps:\n  - step_type: if\n    data:\n      expression: \"document.querySelector('.error') !== null\"\n      then:\n        - step_type: screenshot\n          data:\n            file: \"./error.png\"\n        - step_type: echo\n          data:\n            echo: \"Error found\"\n      else:\n        - step_type: echo\n          data:\n            echo: \"No error\"";
        let config = parse_script(yaml).unwrap();
        match &config.steps[0] {
            Step::If { data } => {
                assert_eq!(data.expression, "document.querySelector('.error') !== null");
                assert_eq!(data.then.len(), 2);
                assert_eq!(data.r#else.len(), 1);
            }
            _ => panic!("expected If step"),
        }
    }

    #[test]
    fn test_parse_retry() {
        let yaml = "steps:\n  - step_type: retry\n    data:\n      count: 3\n      delay: 500\n      steps:\n        - step_type: goto\n          data:\n            goto: \"https://example.com\"\n        - step_type: wait\n          data:\n            wait: \".content\"";
        let config = parse_script(yaml).unwrap();
        match &config.steps[0] {
            Step::Retry { data } => {
                assert_eq!(data.count, 3);
                assert_eq!(data.delay, Some(500));
                assert_eq!(data.steps.len(), 2);
            }
            _ => panic!("expected Retry step"),
        }
    }

    #[test]
    fn test_parse_on_error() {
        let yaml = "on_error:\n  action: continue\n  screenshot: true\nsteps:\n  - step_type: goto\n    data:\n      goto: \"https://example.com\"";
        let config = parse_script(yaml).unwrap();
        assert!(matches!(ErrorAction::Continue, _));
        assert!(config.on_error.screenshot_on_error);
    }

    #[test]
    fn test_parse_new_tab_close_tab() {
        let yaml = "steps:\n  - step_type: new-tab\n    data:\n      url: \"https://example.com\"\n  - step_type: close-tab";
        let config = parse_script(yaml).unwrap();
        assert_eq!(config.steps[0].name(), "new_tab");
        assert_eq!(config.steps[1].name(), "close_tab");
    }

    #[test]
    fn test_parse_empty_steps() {
        let yaml = "steps: []";
        let config = parse_script(yaml).unwrap();
        assert!(config.steps.is_empty());
    }

    #[test]
    fn test_parse_error_missing_steps() {
        let yaml = "name: test";
        let result = parse_script(yaml);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_interactive_steps() {
        let yaml = "name: test\nsteps:\n  - step_type: goto\n    data:\n      goto: \"https://example.com/login\"\n  - step_type: fill\n    data:\n      selector: \"#user\"\n      value: admin\n  - step_type: fill\n    data:\n      selector: \"#pass\"\n      value: secret123\n  - step_type: click\n    data:\n      click: \"button[type=submit]\"\n  - step_type: wait\n    data:\n      wait: \".dashboard\"\n  - step_type: back\n  - step_type: extract\n    data:\n      selector: \".info\"\n      all: true";
        let config = parse_script(yaml).unwrap();
        assert_eq!(config.steps.len(), 7);
        assert_eq!(config.steps[0].name(), "goto");
        assert_eq!(config.steps[3].name(), "click");
        assert_eq!(config.steps[5].name(), "back");
        assert_eq!(config.steps[6].name(), "extract");
    }
}