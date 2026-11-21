//! Plan files — the input to `notchd run-plan`.
//!
//! Shape (both `stuck_after_secs` and `stuckAfterSecs` accepted):
//! ```json
//! {
//!   "stuck_after_secs": 15,
//!   "supervisors": [
//!     { "cwd": "/repo",
//!       "sessions": [ { "title": "Fix login race", "prompt": "…", "cwd": "/repo/api" } ] }
//!   ]
//! }
//! ```
//! Per-session `cwd` falls back to the supervisor's `cwd`. `title` falls back
//! to the prompt head. All cwds must exist and be directories — the plan is
//! rejected cleanly otherwise (no partial spawns from a broken plan).

use serde::Deserialize;
use std::path::Path;

#[derive(Deserialize)]
pub struct PlanFile {
    #[serde(default, alias = "stuckAfterSecs")]
    pub stuck_after_secs: Option<u64>,
    #[serde(default)]
    pub supervisors: Vec<PlanSupervisor>,
}

#[derive(Deserialize)]
pub struct PlanSupervisor {
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub sessions: Vec<PlanSession>,
}

#[derive(Deserialize)]
pub struct PlanSession {
    #[serde(default)]
    pub title: Option<String>,
    pub prompt: String,
    #[serde(default)]
    pub cwd: Option<String>,
}

#[derive(Debug)]
pub struct ParsedSession {
    pub title: String,
    pub prompt: String,
    pub cwd: String,
}

#[derive(Debug)]
pub struct ParsedPlan {
    pub stuck_after_secs: Option<u64>,
    pub supervisors: Vec<Vec<ParsedSession>>,
}

impl ParsedSession {
    pub fn task_head(&self) -> String {
        crate::model::single_line(&self.prompt)
    }
}

pub fn parse_plan(text: &str) -> Result<ParsedPlan, String> {
    let plan: PlanFile =
        serde_json::from_str(text).map_err(|e| format!("invalid plan JSON: {e}"))?;
    let mut supervisors = Vec::with_capacity(plan.supervisors.len());
    for (i, sup) in plan.supervisors.iter().enumerate() {
        let mut sessions = Vec::with_capacity(sup.sessions.len());
        for (j, ses) in sup.sessions.iter().enumerate() {
            let cwd =
                ses.cwd.clone().or_else(|| sup.cwd.clone()).ok_or_else(|| {
                    format!("supervisor {} session {}: missing cwd", i + 1, j + 1)
                })?;
            let cwd_path = Path::new(&cwd);
            if !cwd_path.is_dir() {
                return Err(format!(
                    "supervisor {} session {}: cwd `{}` is not a directory",
                    i + 1,
                    j + 1,
                    cwd
                ));
            }
            let prompt = ses.prompt.trim().to_string();
            if prompt.is_empty() {
                return Err(format!(
                    "supervisor {} session {}: empty prompt",
                    i + 1,
                    j + 1
                ));
            }
            let title = ses
                .title
                .as_deref()
                .map(str::trim)
                .filter(|t| !t.is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| crate::model::single_line(&prompt));
            sessions.push(ParsedSession { title, prompt, cwd });
        }
        supervisors.push(sessions);
    }
    Ok(ParsedPlan {
        stuck_after_secs: plan.stuck_after_secs,
        supervisors,
    })
}

pub fn load_plan(path: &Path) -> Result<ParsedPlan, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read plan `{}`: {e}", path.display()))?;
    parse_plan(&text)
}
