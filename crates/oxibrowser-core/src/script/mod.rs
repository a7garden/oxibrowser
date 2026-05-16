//! Script Runner — parse and execute YAML browser scripts on a Tab.
//!
//! Shared between:
//! - `oxibrowser run <yaml>` (CLI, developer tool)
//! - BrowserTool in oxios-kernel (agent tool, via library)
//!
//! # Example
//!
//! ```ignore
//! let mut runner = ScriptRunner::new(tab);
//! let result = runner.run(yaml_script).await?;
//! for step_result in result.steps {
//!     println!("{:?}", step_result);
//! }
//! ```

pub mod parser;
pub mod runner;
pub mod types;

pub use parser::parse_script;
pub use runner::ScriptRunner;
pub use types::{ErrorStrategy, ScriptConfig, ScriptResult, Step, StepResult};
