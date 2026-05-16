//! Script types — Step enum, ScriptConfig, ScriptResult, StepResult.
//!
//! Defines all types for YAML browser scripts:
//! - `Step` — every action a script can perform
//! - `ScriptConfig` — top-level script metadata (name, timeout, on_error)
//! - `StepResult` — result of a single step execution
//! - `ScriptResult` — aggregate result of running an entire script

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Top-level error handling strategy for a script.
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ErrorAction {
    /// Abort script execution on first error (default).
    #[default]
    Abort,
    /// Continue executing remaining steps on error.
    Continue,
}

impl Default for ErrorStrategy {
    fn default() -> Self {
        Self {
            action: ErrorAction::Abort,
            screenshot_on_error: false,
            retry: None,
        }
    }
}

/// Error handling configuration for a script.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ErrorStrategy {
    /// What to do when a step fails.
    #[serde(rename = "action", default)]
    pub action: ErrorAction,

    /// Take a screenshot when a step fails.
    #[serde(rename = "screenshot", default)]
    pub screenshot_on_error: bool,

    /// Global retry for failed steps.
    #[serde(rename = "retry", default)]
    pub retry: Option<RetryConfig>,
}

/// Retry configuration for step execution.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RetryConfig {
    pub count: usize,
    #[serde(rename = "delay_ms", default)]
    pub delay_ms: u64,
}

/// Top-level script configuration (parsed from YAML).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ScriptConfig {
    /// Human-readable script name.
    pub name: Option<String>,

    /// Global timeout in milliseconds for wait_for steps.
    #[serde(default = "default_timeout")]
    pub timeout: u64,

    /// Error handling strategy.
    #[serde(rename = "on_error", default)]
    pub on_error: ErrorStrategy,

    /// The ordered list of steps to execute.
    pub steps: Vec<Step>,
}

fn default_timeout() -> u64 {
    30000
}

impl Default for ScriptConfig {
    fn default() -> Self {
        Self {
            name: None,
            timeout: 30000,
            on_error: ErrorStrategy::default(),
            steps: Vec::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// Step types
// ---------------------------------------------------------------------------

/// A single script step. Each variant maps to a Tab method or compound action.
///
/// Parsing is done with #[serde(flatten)] + untagged enum so every step variant
/// is identifiable by its key (e.g., `goto:`, `click:`, `fill:`).
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "step_type", rename_all = "lowercase")]
pub enum StepType {
    // Navigation
    Goto,
    Back,
    Forward,
    Reload,
    Post,
    // Interaction
    Click,
    DblClick,
    RightClick,
    Hover,
    Fill,
    Type,
    Clear,
    Check,
    Uncheck,
    Select,
    Press,
    Scroll,
    Drag,
    // Content
    Evaluate,
    Wait,
    Extract,
    Content,
    Screenshot,
    LoadResources,
    // Flow Control
    Set,
    Echo,
    Sleep,
    If,
    Retry,
    // Session
    NewTab,
    CloseTab,
}

impl Step {
    /// Return the step type name for error messages.
    pub fn step_name(&self) -> &str {
        match self {
            Step::Goto { .. } => "goto",
            Step::Back => "back",
            Step::Forward => "forward",
            Step::Reload => "reload",
            Step::Post { .. } => "post",
            Step::Click { .. } => "click",
            Step::DblClick { .. } => "dbl-click",
            Step::RightClick { .. } => "right-click",
            Step::Hover { .. } => "hover",
            Step::Fill { .. } => "fill",
            Step::Type { .. } => "type",
            Step::Clear { .. } => "clear",
            Step::Check { .. } => "check",
            Step::Uncheck { .. } => "uncheck",
            Step::Select { .. } => "select",
            Step::Press { .. } => "press",
            Step::Scroll { .. } => "scroll",
            Step::Drag { .. } => "drag",
            Step::Evaluate { .. } => "evaluate",
            Step::Wait { .. } => "wait",
            Step::Extract { .. } => "extract",
            Step::Content { .. } => "content",
            Step::Screenshot { .. } => "screenshot",
            Step::LoadResources => "load_resources",
            Step::Set { .. } => "set",
            Step::Echo { .. } => "echo",
            Step::Sleep { .. } => "sleep",
            Step::If { .. } => "if",
            Step::Retry { .. } => "retry",
            Step::NewTab { .. } => "new_tab",
            Step::CloseTab => "close_tab",
        }
    }
}

/// Navigation step: go to a URL.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct GotoStep {
    /// URL to navigate to.
    pub goto: String,
    /// Wait for a CSS selector after navigation.
    pub wait: Option<String>,
}

/// Navigation step: HTTP POST.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PostStep {
    /// POST body (raw string).
    pub body: String,
    /// Content-Type header.
    #[serde(rename = "content_type", default = "default_content_type")]
    pub content_type: String,
    /// URL to POST to.
    pub url: String,
}

fn default_content_type() -> String {
    "application/x-www-form-urlencoded".to_string()
}

/// Interaction step: click an element.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ClickStep {
    /// CSS selector of element to click.
    pub click: String,
}

/// Interaction step: fill an input element.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct FillStep {
    /// CSS selector of the input element.
    pub selector: String,
    /// Value to fill in.
    pub value: Value,
}

/// Interaction step: type text into an element.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TypeStep {
    /// CSS selector of the input element.
    pub selector: String,
    /// Text to type.
    pub text: String,
}

/// Interaction step: press a key combo.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PressStep {
    /// Key combo (e.g., "Enter", "Ctrl+C", "Tab").
    pub press: String,
}

/// Interaction step: scroll the viewport.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ScrollStep {
    /// Horizontal scroll delta in pixels.
    pub x: f64,
    /// Vertical scroll delta in pixels.
    pub y: f64,
}

/// Interaction step: drag from one element to another.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DragStep {
    /// CSS selector of the source element.
    pub from: String,
    /// CSS selector of the target element.
    pub to: String,
}

/// Content step: evaluate JavaScript.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct EvaluateStep {
    /// JavaScript expression to evaluate.
    pub evaluate: Value,
    /// Whether to await the result (default: false).
    #[serde(default)]
    pub r#await: bool,
    /// Save the result to a named variable.
    pub save: Option<String>,
}

/// Content step: wait for a selector.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct WaitStep {
    /// CSS selector to wait for.
    pub wait: String,
    /// Wait timeout in milliseconds (default: from config).
    pub timeout: Option<u64>,
}

/// Content step: extract data from the page.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ExtractStep {
    /// CSS selector of elements to extract.
    pub selector: String,
    /// Extract all matching elements (default: false).
    #[serde(default)]
    pub all: bool,
    /// Extract links (href values).
    #[serde(default)]
    pub links: bool,
    /// Extract text content.
    #[serde(default)]
    pub text: bool,
    /// Save the result to a named variable.
    pub save: Option<String>,
}

/// Content step: get page content in a specific format.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ContentStep {
    /// Content format.
    #[serde(rename = "format", default)]
    pub format: String,
    /// Save the result to a named variable.
    pub save: Option<String>,
}

/// Content step: take a screenshot.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ScreenshotStep {
    /// File path to save the PNG. If None, screenshot data is in StepResult.
    pub file: Option<String>,
    /// Screenshot width in pixels (default: 800).
    #[serde(default = "default_screenshot_width")]
    pub width: u32,
}

fn default_screenshot_width() -> u32 {
    800
}

/// Flow control: set a variable.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SetStep {
    /// Variable name and value pairs.
    #[serde(flatten)]
    pub vars: std::collections::HashMap<String, Value>,
}

/// Flow control: print a message.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct EchoStep {
    /// Message to print.
    pub echo: String,
}

/// Flow control: sleep for a duration.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SleepStep {
    /// Sleep duration in milliseconds.
    pub sleep: u64,
}

/// Flow control: conditional execution.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct IfStep {
    /// JavaScript expression to evaluate.
    pub expression: String,
    /// Steps to execute if expression is truthy.
    pub then: Vec<Step>,
    /// Steps to execute if expression is falsy.
    #[serde(default)]
    pub r#else: Vec<Step>,
}

/// Flow control: retry a block of steps.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RetryStep {
    /// Number of retry attempts.
    pub count: usize,
    /// Delay between retries in milliseconds.
    pub delay: Option<u64>,
    /// Steps to execute (and retry).
    pub steps: Vec<Step>,
}

/// Flow control: select option from a <select> element.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SelectStep {
    /// CSS selector of the <select> element.
    pub selector: String,
    /// Option value to select.
    pub value: String,
}

/// Session step: open a new tab.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct NewTabStep {
    /// URL to open in the new tab.
    pub url: Option<String>,
}

/// A single step in a browser script.
///
/// Each variant is parsed from YAML by its identifying key.
// NOTE: Order matters for serde deserialization — fields with no value (e.g., Back)
// are distinguished by absence of other fields.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "step_type", content = "data", rename_all = "lowercase")]
pub enum Step {
    // Navigation
    #[serde(rename = "goto")]
    Goto {
        #[serde(flatten)]
        data: GotoStep,
    },
    #[serde(rename = "back")]
    Back,
    #[serde(rename = "forward")]
    Forward,
    #[serde(rename = "reload")]
    Reload,
    #[serde(rename = "post")]
    Post {
        #[serde(flatten)]
        data: PostStep,
    },

    // Interaction
    #[serde(rename = "click")]
    Click {
        #[serde(flatten)]
        data: ClickStep,
    },
    #[serde(rename = "dbl-click")]
    DblClick {
        #[serde(flatten)]
        data: ClickStep,
    },
    #[serde(rename = "right-click")]
    RightClick {
        #[serde(flatten)]
        data: ClickStep,
    },
    #[serde(rename = "hover")]
    Hover {
        #[serde(flatten)]
        data: ClickStep,
    },
    #[serde(rename = "fill")]
    Fill {
        #[serde(flatten)]
        data: FillStep,
    },
    #[serde(rename = "type")]
    Type {
        #[serde(flatten)]
        data: TypeStep,
    },
    #[serde(rename = "clear")]
    Clear {
        /// CSS selector of element to clear.
        selector: String,
    },
    #[serde(rename = "check")]
    Check {
        /// CSS selector of checkbox/radio to check.
        selector: String,
    },
    #[serde(rename = "uncheck")]
    Uncheck {
        /// CSS selector of checkbox/radio to uncheck.
        selector: String,
    },
    #[serde(rename = "select")]
    Select {
        #[serde(flatten)]
        data: SelectStep,
    },
    #[serde(rename = "press")]
    Press {
        #[serde(flatten)]
        data: PressStep,
    },
    #[serde(rename = "scroll")]
    Scroll {
        #[serde(flatten)]
        data: ScrollStep,
    },
    #[serde(rename = "drag")]
    Drag {
        #[serde(flatten)]
        data: DragStep,
    },

    // Content
    #[serde(rename = "evaluate")]
    Evaluate {
        #[serde(flatten)]
        data: EvaluateStep,
    },
    #[serde(rename = "wait")]
    Wait {
        #[serde(flatten)]
        data: WaitStep,
    },
    #[serde(rename = "extract")]
    Extract {
        #[serde(flatten)]
        data: ExtractStep,
    },
    #[serde(rename = "content")]
    Content {
        #[serde(flatten)]
        data: ContentStep,
    },
    #[serde(rename = "screenshot")]
    Screenshot {
        #[serde(flatten)]
        data: ScreenshotStep,
    },
    #[serde(rename = "load_resources")]
    LoadResources,

    // Flow Control
    #[serde(rename = "set")]
    Set {
        #[serde(flatten)]
        data: SetStep,
    },
    #[serde(rename = "echo")]
    Echo {
        #[serde(flatten)]
        data: EchoStep,
    },
    #[serde(rename = "sleep")]
    Sleep {
        #[serde(flatten)]
        data: SleepStep,
    },
    #[serde(rename = "if")]
    If {
        #[serde(flatten)]
        data: IfStep,
    },
    #[serde(rename = "retry")]
    Retry {
        #[serde(flatten)]
        data: RetryStep,
    },

    // Session
    #[serde(rename = "new-tab")]
    NewTab {
        #[serde(flatten)]
        data: NewTabStep,
    },
    #[serde(rename = "close-tab")]
    CloseTab,
}

impl Step {
    /// Returns the step type name.
    pub fn name(&self) -> &'static str {
        match self {
            Step::Goto { .. } => "goto",
            Step::Back => "back",
            Step::Forward => "forward",
            Step::Reload => "reload",
            Step::Post { .. } => "post",
            Step::Click { .. } => "click",
            Step::DblClick { .. } => "dbl-click",
            Step::RightClick { .. } => "right-click",
            Step::Hover { .. } => "hover",
            Step::Fill { .. } => "fill",
            Step::Type { .. } => "type",
            Step::Clear { .. } => "clear",
            Step::Check { .. } => "check",
            Step::Uncheck { .. } => "uncheck",
            Step::Select { .. } => "select",
            Step::Press { .. } => "press",
            Step::Scroll { .. } => "scroll",
            Step::Drag { .. } => "drag",
            Step::Evaluate { .. } => "evaluate",
            Step::Wait { .. } => "wait",
            Step::Extract { .. } => "extract",
            Step::Content { .. } => "content",
            Step::Screenshot { .. } => "screenshot",
            Step::LoadResources => "load_resources",
            Step::Set { .. } => "set",
            Step::Echo { .. } => "echo",
            Step::Sleep { .. } => "sleep",
            Step::If { .. } => "if",
            Step::Retry { .. } => "retry",
            Step::NewTab { .. } => "new_tab",
            Step::CloseTab => "close_tab",
        }
    }
}

// ---------------------------------------------------------------------------
// Result types
// ---------------------------------------------------------------------------

/// Result of executing a single step.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepResult {
    /// The step index (0-based).
    pub index: usize,
    /// The step type name.
    pub step_type: String,
    /// Step-specific data (extracted text, screenshot bytes, evaluate result, etc.).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
    /// Whether the step succeeded.
    pub success: bool,
    /// Error message if the step failed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Path to screenshot taken on error (if applicable).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_screenshot: Option<String>,
}

impl StepResult {
    /// Create a successful step result.
    pub fn success(index: usize, step_type: &str, data: Option<Value>) -> Self {
        Self {
            index,
            step_type: step_type.to_string(),
            data,
            success: true,
            error: None,
            error_screenshot: None,
        }
    }

    /// Create an error step result.
    pub fn error(index: usize, step_type: &str, error: String) -> Self {
        Self {
            index,
            step_type: step_type.to_string(),
            data: None,
            success: false,
            error: Some(error),
            error_screenshot: None,
        }
    }

    /// Set the error screenshot path.
    pub fn with_error_screenshot(mut self, path: String) -> Self {
        self.error_screenshot = Some(path);
        self
    }
}

/// Result of executing an entire script.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScriptResult {
    /// Name of the script (from config).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Results of each step in execution order.
    pub steps: Vec<StepResult>,
    /// Variables set during script execution.
    pub vars: std::collections::HashMap<String, Value>,
    /// Whether the script completed successfully (no aborted errors).
    pub success: bool,
    /// Total execution time in milliseconds.
    pub duration_ms: u64,
}

impl ScriptResult {
    /// Create a new ScriptResult.
    pub fn new(name: Option<String>) -> Self {
        Self {
            name,
            steps: Vec::new(),
            vars: std::collections::HashMap::new(),
            success: true,
            duration_ms: 0,
        }
    }
}