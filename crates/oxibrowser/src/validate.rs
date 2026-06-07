//! Input validation for agent-submitted CLI arguments.
//!
//! Agents are not trusted operators. Every string input must be validated
//! before being passed to the browser engine.

/// Validation error with a machine-readable code.
#[derive(Debug)]
#[allow(dead_code)]
pub enum InputError {
    ControlChars,
    NullByte,
    Empty,
    PathTraversal,
    InvalidUrl(String),
    InvalidScheme,
    DoubleEncoding,
}

impl std::fmt::Display for InputError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ControlChars => write!(f, "control character detected in input"),
            Self::NullByte => write!(f, "null byte detected in input"),
            Self::Empty => write!(f, "empty input"),
            Self::PathTraversal => write!(f, "path traversal detected: output path escapes CWD"),
            Self::InvalidUrl(msg) => write!(f, "invalid URL: {msg}"),
            Self::InvalidScheme => write!(f, "invalid URL scheme: only http and https are allowed"),
            Self::DoubleEncoding => write!(f, "double encoding detected in URL"),
        }
    }
}

impl std::error::Error for InputError {}

impl InputError {
    /// Machine-readable error code for agent consumption.
    pub fn error_code(&self) -> &'static str {
        match self {
            Self::ControlChars => "INPUT_VALIDATION",
            Self::NullByte => "INPUT_VALIDATION",
            Self::Empty => "INPUT_VALIDATION",
            Self::PathTraversal => "PATH_TRAVERSAL",
            Self::InvalidUrl(_) => "INVALID_URL",
            Self::InvalidScheme => "INVALID_URL",
            Self::DoubleEncoding => "INPUT_VALIDATION",
        }
    }

    /// Exit code for this error (always 2 for input validation).
    #[allow(dead_code)]
    pub fn exit_code(&self) -> i32 {
        crate::output::exit_code::INPUT
    }
}

/// Reject control characters (except newline, CR, tab).
///
/// Agents can hallucinate invisible characters into strings.
pub fn reject_control_chars(input: &str) -> Result<&str, InputError> {
    if input
        .chars()
        .any(|c| (c < '\x20' && c != '\n' && c != '\r' && c != '\t') || c == '\x7f')
    {
        return Err(InputError::ControlChars);
    }
    Ok(input)
}

/// Reject null bytes.
pub fn reject_null_bytes(input: &str) -> Result<&str, InputError> {
    if input.contains('\0') {
        return Err(InputError::NullByte);
    }
    Ok(input)
}

/// Validate a URL string. Only http and https schemes are allowed.
pub fn validate_url(url: &str) -> Result<url::Url, InputError> {
    reject_control_chars(url)?;
    reject_null_bytes(url)?;

    // Check for double encoding (%25 in an already-encoded string)
    if url.contains("%25") && url.matches('%').count() > 1 {
        return Err(InputError::DoubleEncoding);
    }

    let parsed = url::Url::parse(url).map_err(|e| InputError::InvalidUrl(e.to_string()))?;
    match parsed.scheme() {
        "http" | "https" => Ok(parsed),
        _ => Err(InputError::InvalidScheme),
    }
}

/// Validate a CSS selector string.
pub fn validate_selector(selector: &str) -> Result<&str, InputError> {
    reject_null_bytes(selector)?;
    reject_control_chars(selector)?;
    if selector.trim().is_empty() {
        return Err(InputError::Empty);
    }
    Ok(selector)
}

/// Validate a JS expression string.
pub fn validate_expression(expr: &str) -> Result<&str, InputError> {
    reject_null_bytes(expr)?;
    reject_control_chars(expr)?;
    Ok(expr)
}

/// Validate an output file path stays within the CWD.
#[allow(dead_code)]
pub fn safe_output_path(path: &str) -> Result<std::path::PathBuf, InputError> {
    reject_null_bytes(path)?;
    reject_control_chars(path)?;

    let cwd = std::env::current_dir().unwrap_or_default();
    let resolved = cwd.join(path);

    // Canonicalize the parent directory (file may not exist yet)
    let parent = resolved.parent().unwrap_or(&cwd);
    let canonical_parent = match parent.canonicalize() {
        Ok(p) => p,
        Err(_) => return Err(InputError::PathTraversal),
    };

    let canonical_cwd = cwd.canonicalize().unwrap_or(cwd);
    if !canonical_parent.starts_with(&canonical_cwd) {
        return Err(InputError::PathTraversal);
    }

    Ok(resolved)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_reject_control_chars_ok() {
        assert!(reject_control_chars("hello world").is_ok());
        assert!(reject_control_chars("hello\nworld").is_ok());
        assert!(reject_control_chars("hello\tworld").is_ok());
    }

    #[test]
    fn test_reject_control_chars_rejects() {
        assert!(reject_control_chars("hello\x01world").is_err());
        assert!(reject_control_chars("hello\x7fworld").is_err());
        assert!(reject_control_chars("hello\x00world").is_err());
    }

    #[test]
    fn test_validate_url_ok() {
        assert!(validate_url("https://example.com").is_ok());
        assert!(validate_url("http://example.com/path?q=1").is_ok());
    }

    #[test]
    fn test_validate_url_rejects_bad_scheme() {
        assert!(validate_url("file:///etc/passwd").is_err());
        assert!(validate_url("javascript:alert(1)").is_err());
        assert!(validate_url("ftp://example.com").is_err());
    }

    #[test]
    fn test_validate_url_rejects_garbage() {
        assert!(validate_url("not-a-url").is_err());
    }

    #[test]
    fn test_validate_selector_ok() {
        assert!(validate_selector("div.class").is_ok());
        assert!(validate_selector("#id").is_ok());
        assert!(validate_selector("a[href]").is_ok());
    }

    #[test]
    fn test_validate_selector_rejects() {
        assert!(validate_selector("").is_err());
        assert!(validate_selector("  ").is_err());
        assert!(validate_selector("div\0.class").is_err());
        assert!(validate_selector("div\x01").is_err());
    }

    #[test]
    fn test_validate_expression_ok() {
        assert!(validate_expression("document.title").is_ok());
        assert!(validate_expression("1 + 2").is_ok());
    }

    #[test]
    fn test_validate_expression_rejects() {
        assert!(validate_expression("alert\x01(1)").is_err());
        assert!(validate_expression("foo\0bar").is_err());
    }

    #[test]
    fn test_safe_output_path_ok() {
        assert!(safe_output_path("output.png").is_ok());
        assert!(
            safe_output_path("output.png")
                .unwrap()
                .ends_with("output.png")
        );
    }

    #[test]
    fn test_safe_output_path_rejects_traversal() {
        assert!(safe_output_path("../../etc/passwd").is_err());
        assert!(safe_output_path("/etc/passwd").is_err());
    }
}
