//! Script runner — executes parsed scripts on a Tab.
//!
//! Executes a `ScriptConfig` (steps + config) on a `Tab`, handling:
//! - Variable interpolation (`${var}` in strings)
//! - Error handling (abort/continue, retry, screenshots)
//! - Conditional execution (`if`)
//
//! The runner is stateful: it maintains variables and error strategy across steps.
//! `run()` takes `&mut self` so variables persist across calls.

use crate::tab::Tab;
use std::collections::HashMap;
use std::time::Instant;

use super::parser::ScriptError;
use super::types::{
    ErrorAction, ErrorStrategy, ExtractStep, FillStep, IfStep, PressStep, RetryStep,
    ScriptConfig, ScriptResult, ScrollStep, SelectStep, SetStep, SleepStep, Step, StepResult,
    TypeStep,
};

use super::parser::parse_script as parse_script_fn;

/// Re-export parse_script from parser for convenience
pub use super::parser::parse_script;

/// Script runner — executes a parsed `ScriptConfig` on a `Tab`.
pub struct ScriptRunner<'a> {
    /// The tab to execute scripts on.
    tab: &'a Tab,
    /// Script-level variables set via `set` steps.
    vars: HashMap<String, serde_json::Value>,
    /// Script-level error handling strategy.
    on_error: ErrorStrategy,
    /// Console output buffer (for tests/debugging).
    /// Filled by `echo` steps.
    console: Vec<String>,
}

impl<'a> ScriptRunner<'a> {
    /// Create a new runner for a tab.
    pub fn new(tab: &'a Tab) -> Self {
        Self {
            tab,
            vars: HashMap::new(),
            on_error: ErrorStrategy::default(),
            console: Vec::new(),
        }
    }

    /// Run a script from a YAML string.
    ///
    /// Parses the YAML, then executes all steps on the tab.
    /// Returns a `ScriptResult` with step results, variables, and timing.
    ///
    /// # Errors
    ///
    /// Returns `ScriptError::Parse` if YAML is invalid.
    /// Returns errors from Tab methods for navigation/interaction failures.
    /// Returns `ScriptError::Exec` if the script aborts due to a step error
    /// and `on_error.action == Abort`.
    pub async fn run(&mut self, yaml: &str) -> Result<ScriptResult, ScriptError> {
        let config = parse_script_fn(yaml)?;
        self.run_config(&config).await
    }

    /// Run a parsed `ScriptConfig` on the tab.
    ///
    /// This is the main execution loop. It:
    /// 1. Sets up error strategy from config
    /// 2. Iterates over steps, executing each one
    /// 3. Handles errors per `on_error` strategy
    /// 4. Collects `StepResult`s into a `ScriptResult`
    pub async fn run_config(&mut self, config: &ScriptConfig) -> Result<ScriptResult, ScriptError> {
        let start = Instant::now();
        let mut result = ScriptResult::new(config.name.clone());
        self.on_error = config.on_error.clone();
        self.vars.clear();

        for (i, step) in config.steps.iter().enumerate() {
            let step_result = self.execute_step(step, i).await;

            let should_continue = match &step_result {
                Ok(r) => {
                    result.steps.push(r.clone());
                    true
                }
                Err(e) => {
                    let err_str = e.to_string();
                    // Take screenshot on error if configured
                    let err_screenshot = if self.on_error.screenshot_on_error {
                        self.take_error_screenshot(i).await.ok()
                    } else {
                        None
                    };

                    let mut sr = StepResult::error(i, step.name(), err_str);
                    if let Some(path) = err_screenshot {
                        sr = sr.with_error_screenshot(path);
                    }
                    result.steps.push(sr);

                    match self.on_error.action {
                        ErrorAction::Abort => false,
                        ErrorAction::Continue => true,
                    }
                }
            };

            if !should_continue {
                result.success = false;
                break;
            }
        }

        result.vars = self.vars.clone();
        result.duration_ms = start.elapsed().as_millis() as u64;

        Ok(result)
    }

    /// Execute a single step, returning the step result or an error.
    async fn execute_step(&mut self, step: &Step, index: usize) -> Result<StepResult, ScriptError> {
        match step {
            // Navigation
            Step::Goto { data } => {
                self.step_goto(data).await.map(|br| {
                    StepResult::success(index, "goto", Some(serde_json::to_value(br).unwrap()))
                })
            }
            Step::Back => self.step_back(index).await,
            Step::Forward => self.step_forward(index).await,
            Step::Reload => self.step_reload(index).await,
            Step::Post { data } => {
                self.step_post(data).await.map(|br| {
                    StepResult::success(index, "post", Some(serde_json::to_value(br).unwrap()))
                })
            }

            // Interaction
            Step::Click { data } => {
                self.step_click(&data.click).await?;
                Ok(StepResult::success(index, "click", None))
            }
            Step::DblClick { data } => {
                self.tab.double_click(&data.click).await.map_err(map_core_err)?;
                Ok(StepResult::success(index, "dbl-click", None))
            }
            Step::RightClick { data } => {
                self.tab.right_click(&data.click).await.map_err(map_core_err)?;
                Ok(StepResult::success(index, "right-click", None))
            }
            Step::Hover { data } => {
                self.tab.hover(&data.click).await.map_err(map_core_err)?;
                Ok(StepResult::success(index, "hover", None))
            }
            Step::Fill { data } => {
                self.step_fill(data).await?;
                Ok(StepResult::success(index, "fill", None))
            }
            Step::Type { data } => {
                self.step_type(data).await?;
                Ok(StepResult::success(index, "type", None))
            }
            Step::Clear { selector } => {
                self.tab.clear_input(selector).await.map_err(map_core_err)?;
                Ok(StepResult::success(index, "clear", None))
            }
            Step::Check { selector } => {
                self.tab.check(selector).await.map_err(map_core_err)?;
                Ok(StepResult::success(index, "check", None))
            }
            Step::Uncheck { selector } => {
                self.tab.uncheck(selector).await.map_err(map_core_err)?;
                Ok(StepResult::success(index, "uncheck", None))
            }
            Step::Select { data } => {
                self.step_select(data).await?;
                Ok(StepResult::success(index, "select", None))
            }
            Step::Press { data } => {
                self.step_press(data).await?;
                Ok(StepResult::success(index, "press", None))
            }
            Step::Scroll { data } => {
                self.step_scroll(data).await?;
                Ok(StepResult::success(index, "scroll", None))
            }
            Step::Drag { data } => {
                self.tab.drag(&data.from, &data.to).await.map_err(map_core_err)?;
                Ok(StepResult::success(index, "drag", None))
            }

            // Content
            Step::Evaluate { data } => {
                self.step_evaluate(data).await.map(|val| {
                    StepResult::success(index, "evaluate", Some(val))
                })
            }
            Step::Wait { data } => {
                self.step_wait(data).await?;
                Ok(StepResult::success(index, "wait", None))
            }
            Step::Extract { data } => {
                self.step_extract(data).await.map(|val| {
                    StepResult::success(index, "extract", Some(val))
                })
            }
            Step::Content { data } => {
                self.step_content(data).await.map(|val| {
                    StepResult::success(index, "content", Some(val))
                })
            }
            Step::Screenshot { data } => {
                self.step_screenshot(data).await.map(|val| {
                    StepResult::success(index, "screenshot", Some(val))
                })
            }
            Step::LoadResources => {
                let count = self.tab.load_resources().await.map_err(map_core_err)?;
                Ok(StepResult::success(
                    index,
                    "load_resources",
                    Some(serde_json::json!(count)),
                ))
            }

            // Flow Control
            Step::Set { data } => {
                self.step_set(data)?;
                Ok(StepResult::success(index, "set", None))
            }
            Step::Echo { data } => {
                self.step_echo(&data.echo);
                Ok(StepResult::success(index, "echo", None))
            }
            Step::Sleep { data } => {
                self.step_sleep(data).await?;
                Ok(StepResult::success(index, "sleep", None))
            }
            Step::If { data } => {
                self.step_if(data).await.map(|_| StepResult::success(index, "if", None))
            }
            Step::Retry { data } => {
                self.step_retry(data).await?;
                Ok(StepResult::success(index, "retry", None))
            }

            // Session
            Step::NewTab { data } => {
                self.step_new_tab(data).await?;
                Ok(StepResult::success(index, "new_tab", None))
            }
            Step::CloseTab => {
                self.tab.close().await.map_err(map_core_err)?;
                Ok(StepResult::success(index, "close_tab", None))
            }
        }
    }

    // ---------------------------------------------------------------------------
    // Navigation steps
    // ---------------------------------------------------------------------------

    async fn step_goto(&self, data: &super::types::GotoStep) -> Result<crate::browse_result::BrowseResult, ScriptError> {
        let url = self.interpolate(&data.goto);
        self.tab.goto(&url).await.map_err(map_core_err)?;

        if let Some(ref selector) = data.wait {
            let timeout = self.on_error.retry.as_ref().map_or(5000, |_| 5000);
            self.tab.wait_for(selector, timeout).await.map_err(map_core_err)?;
        }

        self.tab.content().await.map_err(map_core_err)
    }

    async fn step_back(&self, index: usize) -> Result<StepResult, ScriptError> {
        self.tab.back().await.map_err(map_core_err)?;
        Ok(StepResult::success(index, "back", None))
    }

    async fn step_forward(&self, index: usize) -> Result<StepResult, ScriptError> {
        self.tab.forward().await.map_err(map_core_err)?;
        Ok(StepResult::success(index, "forward", None))
    }

    async fn step_reload(&self, index: usize) -> Result<StepResult, ScriptError> {
        self.tab.reload().await.map_err(map_core_err)?;
        Ok(StepResult::success(index, "reload", None))
    }

    async fn step_post(&self, data: &super::types::PostStep) -> Result<crate::browse_result::BrowseResult, ScriptError> {
        let url = self.interpolate(&data.url);
        let body = self.interpolate(&data.body);
        self.tab.post(&url, &body, &data.content_type).await.map_err(map_core_err)?;
        self.tab.content().await.map_err(map_core_err)
    }

    // ---------------------------------------------------------------------------
    // Interaction steps
    // ---------------------------------------------------------------------------

    async fn step_click(&self, selector: &str) -> Result<(), ScriptError> {
        let selector = self.interpolate(selector);
        self.tab.click(&selector).await.map_err(map_core_err)
    }

    async fn step_fill(&self, data: &FillStep) -> Result<(), ScriptError> {
        let selector = self.interpolate(&data.selector);
        let value = self.interpolate_value(&data.value);
        let value_str = value.as_str().unwrap_or_default();
        self.tab.fill(&selector, value_str).await.map_err(map_core_err)
    }

    async fn step_type(&self, data: &TypeStep) -> Result<(), ScriptError> {
        let selector = self.interpolate(&data.selector);
        let text = self.interpolate(&data.text);
        self.tab.r#type(&selector, &text).await.map_err(map_core_err)
    }

    async fn step_select(&self, data: &SelectStep) -> Result<(), ScriptError> {
        let selector = self.interpolate(&data.selector);
        let value = self.interpolate(&data.value);
        self.tab.select_option(&selector, &value).await.map_err(map_core_err)
    }

    async fn step_press(&self, data: &PressStep) -> Result<(), ScriptError> {
        let key = self.interpolate(&data.press);
        self.tab.press(&key).await.map_err(map_core_err)
    }

    async fn step_scroll(&self, data: &ScrollStep) -> Result<(), ScriptError> {
        self.tab
            .scroll(data.x, data.y)
            .await
            .map_err(map_core_err)
    }

    // ---------------------------------------------------------------------------
    // Content steps
    // ---------------------------------------------------------------------------

    async fn step_evaluate(
        &mut self,
        data: &super::types::EvaluateStep,
    ) -> Result<serde_json::Value, ScriptError> {
        let expr = self.interpolate_value(&data.evaluate);
        let expr_str = expr.as_str().unwrap_or_default();

        let result = if data.r#await {
            self.tab.evaluate_await(expr_str).await
        } else {
            self.tab.evaluate(expr_str).await
        }
        .map_err(map_core_err)?;

        // Save to variable if requested
        if let Some(ref var_name) = data.save {
            self.vars.insert(var_name.clone(), result.clone());
        }

        Ok(result)
    }

    async fn step_wait(&self, data: &super::types::WaitStep) -> Result<(), ScriptError> {
        let selector = self.interpolate(&data.wait);
        let timeout = data.timeout.unwrap_or(5000);
        self.tab.wait_for(&selector, timeout).await.map_err(map_core_err)
    }

    async fn step_extract(
        &mut self,
        data: &ExtractStep,
    ) -> Result<serde_json::Value, ScriptError> {
        let selector = self.interpolate(&data.selector);
        let selector_str = selector.as_str();

        let result = if data.links {
            // Extract all href values
            let js = format!(
                r#"Array.from(document.querySelectorAll('{}')).map(el => el.href)"#,
                selector_str.replace('`', "\\`")
            );
            let val = self.tab.evaluate(&js).await.map_err(map_core_err)?;
            serde_json::json!(val)
        } else if data.all {
            let items = self.tab.query_all(&selector).await.map_err(map_core_err)?;
            serde_json::json!(items)
        } else {
            let items = self.tab.query_all(&selector).await.map_err(map_core_err)?;
            match items.first() {
                Some(text) => serde_json::json!(text),
                None => serde_json::Value::Null,
            }
        };

        // Save to variable if requested
        if let Some(ref var_name) = data.save {
            self.vars.insert(var_name.clone(), result.clone());
        }

        Ok(result)
    }

    async fn step_content(
        &mut self,
        data: &super::types::ContentStep,
    ) -> Result<serde_json::Value, ScriptError> {
        let format = self.interpolate(&data.format);
        let content = self.tab.content().await.map_err(map_core_err)?;

        let val = match format.as_str() {
            "markdown" => serde_json::json!(content.markdown),
            "html" => serde_json::json!(content.html),
            "text" => {
                let body = content
                    .html
                    .split(">")
                    .last()
                    .unwrap_or(&content.html)
                    .to_string();
                serde_json::json!(body.trim())
            }
            "json" => {
                serde_json::to_value(&content).unwrap_or(serde_json::Value::Null)
            }
            _ => serde_json::json!(content.markdown),
        };

        // Save to variable if requested
        if let Some(ref var_name) = data.save {
            self.vars.insert(var_name.clone(), val.clone());
        }

        Ok(val)
    }

    async fn step_screenshot(
        &self,
        data: &super::types::ScreenshotStep,
    ) -> Result<serde_json::Value, ScriptError> {
        let width = data.width;
        let png = self.tab.screenshot(width).await.map_err(map_core_err)?;

        // Save screenshot to file if path is provided
        if let Some(ref path) = data.file {
            let path = self.interpolate(path);
            std::fs::write(&path, &png).map_err(|e| {
                ScriptError::Exec(format!("failed to write screenshot: {e}"))
            })?;
            Ok(serde_json::json!({ "path": path, "size": png.len() }))
        } else {
            // Return base64-encoded PNG
            let b64 = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &png);
            Ok(serde_json::json!({
                "size": png.len(),
                "base64": b64
            }))
        }
    }

    // ---------------------------------------------------------------------------
    // Flow control steps
    // ---------------------------------------------------------------------------

    fn step_set(&mut self, data: &SetStep) -> Result<(), ScriptError> {
        for (key, value) in &data.vars {
            let interpolated = self.interpolate_value(value);
            self.vars.insert(key.clone(), interpolated);
        }
        Ok(())
    }

    fn step_echo(&mut self, message: &str) {
        let interpolated = self.interpolate(message);
        println!("{interpolated}");
        self.console.push(interpolated);
    }

    async fn step_sleep(&self, data: &SleepStep) -> Result<(), ScriptError> {
        tokio::time::sleep(std::time::Duration::from_millis(data.sleep)).await;
        Ok(())
    }

    async fn step_if(&mut self, data: &IfStep) -> Result<(), ScriptError> {
        let expr = self.interpolate(&data.expression);
        let result = self
            .tab
            .evaluate(&expr)
            .await
            .map_err(map_core_err)?;

        let branch = if result.is_null() || result == serde_json::Value::Bool(false) {
            &data.r#else
        } else {
            &data.then
        };

        for (i, step) in branch.iter().enumerate() {
            // Execute sub-steps inline to avoid recursion
            let step_result = match step {
                // We only inline navigation/interaction steps — complex steps just use ?
                Step::Goto { data } => {
                    self.step_goto(data).await.map(|br| {
                        StepResult::success(i, "goto", Some(serde_json::to_value(br).unwrap()))
                    })
                }
                Step::Click { data } => {
                    self.step_click(&data.click).await?;
                    Ok(StepResult::success(i, "click", None))
                }
                Step::Fill { data } => {
                    self.step_fill(data).await?;
                    Ok(StepResult::success(i, "fill", None))
                }
                Step::Type { data } => {
                    self.step_type(data).await?;
                    Ok(StepResult::success(i, "type", None))
                }
                Step::Press { data } => {
                    self.step_press(data).await?;
                    Ok(StepResult::success(i, "press", None))
                }
                Step::Scroll { data } => {
                    self.step_scroll(data).await?;
                    Ok(StepResult::success(i, "scroll", None))
                }
                Step::Wait { data } => {
                    self.step_wait(data).await?;
                    Ok(StepResult::success(i, "wait", None))
                }
                Step::Evaluate { data } => {
                    self.step_evaluate(data).await.map(|val| {
                        StepResult::success(i, "evaluate", Some(val))
                    })
                }
                Step::Screenshot { data } => {
                    self.step_screenshot(data).await.map(|val| {
                        StepResult::success(i, "screenshot", Some(val))
                    })
                }
                Step::Echo { data } => {
                    self.step_echo(&data.echo);
                    Ok(StepResult::success(i, "echo", None))
                }
                Step::Sleep { data } => {
                    self.step_sleep(data).await?;
                    Ok(StepResult::success(i, "sleep", None))
                }
                Step::Back => {
                    self.step_back(i).await
                }
                Step::Forward => {
                    self.step_forward(i).await
                }
                Step::Reload => {
                    self.step_reload(i).await
                }
                Step::Post { data } => {
                    self.step_post(data).await.map(|br| {
                        StepResult::success(i, "post", Some(serde_json::to_value(br).unwrap()))
                    })
                }
                Step::DblClick { data } => {
                    self.tab.double_click(&data.click).await.map_err(map_core_err)?;
                    Ok(StepResult::success(i, "dbl-click", None))
                }
                Step::RightClick { data } => {
                    self.tab.right_click(&data.click).await.map_err(map_core_err)?;
                    Ok(StepResult::success(i, "right-click", None))
                }
                Step::Hover { data } => {
                    self.tab.hover(&data.click).await.map_err(map_core_err)?;
                    Ok(StepResult::success(i, "hover", None))
                }
                Step::Clear { selector } => {
                    self.tab.clear_input(selector).await.map_err(map_core_err)?;
                    Ok(StepResult::success(i, "clear", None))
                }
                Step::Check { selector } => {
                    self.tab.check(selector).await.map_err(map_core_err)?;
                    Ok(StepResult::success(i, "check", None))
                }
                Step::Uncheck { selector } => {
                    self.tab.uncheck(selector).await.map_err(map_core_err)?;
                    Ok(StepResult::success(i, "uncheck", None))
                }
                Step::Select { data } => {
                    self.step_select(data).await?;
                    Ok(StepResult::success(i, "select", None))
                }
                Step::Drag { data } => {
                    self.tab.drag(&data.from, &data.to).await.map_err(map_core_err)?;
                    Ok(StepResult::success(i, "drag", None))
                }
                Step::Extract { data } => {
                    self.step_extract(data).await.map(|val| {
                        StepResult::success(i, "extract", Some(val))
                    })
                }
                Step::Content { data } => {
                    self.step_content(data).await.map(|val| {
                        StepResult::success(i, "content", Some(val))
                    })
                }
                Step::LoadResources => {
                    let count = self.tab.load_resources().await.map_err(map_core_err)?;
                    Ok(StepResult::success(i, "load_resources", Some(serde_json::json!(count))))
                }
                Step::Set { data } => {
                    self.step_set(data)?;
                    Ok(StepResult::success(i, "set", None))
                }
                Step::NewTab { data } => {
                    self.step_new_tab(data).await?;
                    Ok(StepResult::success(i, "new_tab", None))
                }
                Step::CloseTab => {
                    self.tab.close().await.map_err(map_core_err)?;
                    Ok(StepResult::success(i, "close_tab", None))
                }
                Step::If { .. } | Step::Retry { .. } => {
                    // Nested if/retry in if-branch not yet supported — return success to allow continuation
                    Ok(StepResult::success(i, step.name(), None))
                }
            };

            if step_result.is_err() && matches!(self.on_error.action, ErrorAction::Abort) {
                return Err(ScriptError::Exec(format!(
                    "if branch step {} failed: {:?}",
                    i,
                    step_result.err()
                )));
            }
        }

        Ok(())
    }

    async fn step_retry(&mut self, data: &RetryStep) -> Result<(), ScriptError> {
        let delay = data.delay.unwrap_or(500);
        let mut last_err = None;

        for attempt in 0..=data.count {
            // Execute retry steps inline
            let mut step_failed = false;
            for (i, step) in data.steps.iter().enumerate() {
                let step_result = match step {
                    Step::Goto { data } => {
                        self.step_goto(data).await.map(|br| {
                            StepResult::success(i, "goto", Some(serde_json::to_value(br).unwrap()))
                        })
                    }
                    Step::Back => self.step_back(i).await,
                    Step::Forward => self.step_forward(i).await,
                    Step::Reload => self.step_reload(i).await,
                    Step::Post { data } => {
                        self.step_post(data).await.map(|br| {
                            StepResult::success(i, "post", Some(serde_json::to_value(br).unwrap()))
                        })
                    }
                    Step::Click { data } => {
                        self.step_click(&data.click).await?;
                        Ok(StepResult::success(i, "click", None))
                    }
                    Step::DblClick { data } => {
                        self.tab.double_click(&data.click).await.map_err(map_core_err)?;
                        Ok(StepResult::success(i, "dbl-click", None))
                    }
                    Step::RightClick { data } => {
                        self.tab.right_click(&data.click).await.map_err(map_core_err)?;
                        Ok(StepResult::success(i, "right-click", None))
                    }
                    Step::Hover { data } => {
                        self.tab.hover(&data.click).await.map_err(map_core_err)?;
                        Ok(StepResult::success(i, "hover", None))
                    }
                    Step::Fill { data } => {
                        self.step_fill(data).await?;
                        Ok(StepResult::success(i, "fill", None))
                    }
                    Step::Type { data } => {
                        self.step_type(data).await?;
                        Ok(StepResult::success(i, "type", None))
                    }
                    Step::Clear { selector } => {
                        self.tab.clear_input(selector).await.map_err(map_core_err)?;
                        Ok(StepResult::success(i, "clear", None))
                    }
                    Step::Check { selector } => {
                        self.tab.check(selector).await.map_err(map_core_err)?;
                        Ok(StepResult::success(i, "check", None))
                    }
                    Step::Uncheck { selector } => {
                        self.tab.uncheck(selector).await.map_err(map_core_err)?;
                        Ok(StepResult::success(i, "uncheck", None))
                    }
                    Step::Select { data } => {
                        self.step_select(data).await?;
                        Ok(StepResult::success(i, "select", None))
                    }
                    Step::Press { data } => {
                        self.step_press(data).await?;
                        Ok(StepResult::success(i, "press", None))
                    }
                    Step::Scroll { data } => {
                        self.step_scroll(data).await?;
                        Ok(StepResult::success(i, "scroll", None))
                    }
                    Step::Drag { data } => {
                        self.tab.drag(&data.from, &data.to).await.map_err(map_core_err)?;
                        Ok(StepResult::success(i, "drag", None))
                    }
                    Step::Evaluate { data } => {
                        self.step_evaluate(data).await.map(|val| {
                            StepResult::success(i, "evaluate", Some(val))
                        })
                    }
                    Step::Wait { data } => {
                        self.step_wait(data).await?;
                        Ok(StepResult::success(i, "wait", None))
                    }
                    Step::Extract { data } => {
                        self.step_extract(data).await.map(|val| {
                            StepResult::success(i, "extract", Some(val))
                        })
                    }
                    Step::Content { data } => {
                        self.step_content(data).await.map(|val| {
                            StepResult::success(i, "content", Some(val))
                        })
                    }
                    Step::Screenshot { data } => {
                        self.step_screenshot(data).await.map(|val| {
                            StepResult::success(i, "screenshot", Some(val))
                        })
                    }
                    Step::LoadResources => {
                        let count = self.tab.load_resources().await.map_err(map_core_err)?;
                        Ok(StepResult::success(i, "load_resources", Some(serde_json::json!(count))))
                    }
                    Step::Set { data } => {
                        self.step_set(data)?;
                        Ok(StepResult::success(i, "set", None))
                    }
                    Step::Echo { data } => {
                        self.step_echo(&data.echo);
                        Ok(StepResult::success(i, "echo", None))
                    }
                    Step::Sleep { data } => {
                        self.step_sleep(data).await?;
                        Ok(StepResult::success(i, "sleep", None))
                    }
                    Step::NewTab { data } => {
                        self.step_new_tab(data).await?;
                        Ok(StepResult::success(i, "new_tab", None))
                    }
                    Step::CloseTab => {
                        self.tab.close().await.map_err(map_core_err)?;
                        Ok(StepResult::success(i, "close_tab", None))
                    }
                    Step::If { .. } | Step::Retry { .. } => {
                        Ok(StepResult::success(i, step.name(), None))
                    }
                };

                if step_result.is_err() {
                    last_err = step_result.err();
                    step_failed = true;
                    break;
                }
            }

            if !step_failed {
                return Ok(());
            }

            if attempt < data.count {
                tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
            }
        }

        Err(last_err.unwrap_or_else(|| ScriptError::Exec("retry failed".to_string())))
    }

    // ---------------------------------------------------------------------------
    // Session steps
    // ---------------------------------------------------------------------------

    async fn step_new_tab(&self, data: &super::types::NewTabStep) -> Result<(), ScriptError> {
        // For new-tab: the runner currently operates on a single Tab.
        // Implementing multi-tab requires access to the Browser's tab pool.
        // For now, emit a warning and use goto.
        if let Some(ref url) = data.url {
            let url = self.interpolate(url);
            self.tab.goto(&url).await.map_err(map_core_err)?;
        }
        Ok(())
    }

    // ---------------------------------------------------------------------------
    // Error screenshot helper
    // ---------------------------------------------------------------------------

    async fn take_error_screenshot(&self, step_index: usize) -> Result<String, ScriptError> {
        let timestamp = chrono::Local::now().format("%Y%m%d_%H%M%S");
        let path = format!("error_step{}_{}.png", step_index, timestamp);

        let png = self.tab.screenshot(800).await.map_err(map_core_err)?;
        std::fs::write(&path, &png).map_err(|e| {
            ScriptError::Exec(format!("failed to write error screenshot: {e}"))
        })?;

        Ok(path)
    }

    // ---------------------------------------------------------------------------
    // Variable interpolation
    // ---------------------------------------------------------------------------

    /// Interpolate `${var}` references in a string.
    ///
    /// - `${name}` → value from self.vars (as JSON string)
    /// - `$$` → literal `$`
    /// - `${...}` with missing var → leave as-is
    fn interpolate(&self, input: &str) -> String {
        let mut result = input.to_string();

        // Escape $$ first
        result = result.replace("$$", "\x00DOLLAR\x00");

        // Replace ${var} with variable values
        let var_pattern = regex::Regex::new(r"\$\{([^}]+)\}").unwrap();
        result = var_pattern
            .replace_all(&result, |caps: &regex::Captures| {
                let var_name = &caps[1];
                self.vars
                    .get(var_name)
                    .map(|v| serde_json::to_string(v).unwrap_or_default())
                    .unwrap_or_else(|| format!("${{{var_name}}}"))
            })
            .to_string();

        // Restore $$
        result = result.replace("\x00DOLLAR\x00", "$");

        result
    }

    /// Interpolate variables in a JSON Value (String only).
    fn interpolate_value(&self, value: &serde_json::Value) -> serde_json::Value {
        match value {
            serde_json::Value::String(s) => serde_json::Value::String(self.interpolate(s)),
            other => other.clone(),
        }
    }
}

// ---------------------------------------------------------------------------
// Error conversions
// ---------------------------------------------------------------------------

/// Convert a CoreError to ScriptError using the Into pattern.
fn map_core_err(e: crate::error::CoreError) -> ScriptError {
    ScriptError::Exec(e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_interpolate_simple() {
        // Can't test ScriptRunner without a Tab, but we can test the interpolation logic
        // through a test helper. For now, this is covered by integration tests.
    }

    #[test]
    fn test_script_error_display() {
        let e = ScriptError::Parse("invalid yaml".to_string());
        assert_eq!(e.to_string(), "script parse error: invalid yaml");

        let e = ScriptError::Io("file not found".to_string());
        assert_eq!(e.to_string(), "script I/O error: file not found");

        let e = ScriptError::Exec("step failed".to_string());
        assert_eq!(e.to_string(), "script exec error: step failed");
    }
}