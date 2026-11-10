//! Each agent's usage from its own records: prompts, steps, tokens, models, tools
//! and recent sessions, plus its provider's quota when the user turned that on
//! (see `quota`). Records are read incrementally (see `logcache`), so a rescan
//! costs only what was written since the last one.

pub mod claude;
pub mod codex;
pub mod logcache;
pub mod procs;
pub mod quota;
pub mod time;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use nyx_agents_proto::{ModelUsage, ToolUsage, Usage, UsageDay, UsageSession};

/// The agents with a usage scanner.
pub const AGENTS: &[&str] = &["claude", "codex"];

/// Past sessions kept per agent for the session history.
pub const HISTORY: usize = 50;

/// `~/.cache/nyx-agents/usage/<agent>`.
pub fn cache_dir(agent: &str) -> PathBuf {
    crate::paths::cache_dir().join("usage").join(agent)
}

/// Scan one agent. `refresh` also fetches its quota now, ignoring the retry wait.
pub fn scan(agent: &str, refresh: bool) -> Option<Usage> {
    scan_with_history(agent, refresh).map(|(u, _)| u)
}

/// Scan one agent: its usage, and its past sessions (newest first, at most `HISTORY`).
pub fn scan_with_history(agent: &str, refresh: bool) -> Option<(Usage, Vec<UsageSession>)> {
    let (mut u, history) = match agent {
        "claude" => claude::scan(&claude::base(), refresh),
        "codex" => codex::scan(&codex::base(), refresh),
        _ => return None,
    };
    u.updated = time::now() as u64;
    Some((u, history))
}

/// Every agent that has records or a quota, last used first.
pub fn scan_all(refresh: bool) -> Vec<Usage> {
    let mut all: Vec<Usage> = AGENTS.iter().filter_map(|a| scan(a, refresh)).filter(|u| u.has_records || u.quota.enabled).collect();
    sort(&mut all);
    all
}

/// Working agents first, then last used first.
pub fn sort(all: &mut [Usage]) {
    all.sort_by(|a, b| b.working.cmp(&a.working).then(b.last_used.cmp(&a.last_used)).then(a.name.cmp(&b.name)));
}

/// Counts by day, model and tool, shared by the scanners.
pub struct Tally {
    today: String,
    days: Vec<String>,
    week: HashSet<String>,
    prompts_by_day: HashMap<String, u64>,
    steps_by_day: HashMap<String, u64>,
    models: HashMap<String, ModelUsage>,
    tools: HashMap<String, u64>,
    pub today_tokens: u64,
    pub today_cached: u64,
}

impl Default for Tally {
    fn default() -> Self {
        let days = time::last_days(7);
        Self {
            today: days[6].clone(),
            week: days.iter().cloned().collect(),
            days,
            prompts_by_day: HashMap::new(),
            steps_by_day: HashMap::new(),
            models: HashMap::new(),
            tools: HashMap::new(),
            today_tokens: 0,
            today_cached: 0,
        }
    }
}

impl Tally {
    fn model(&mut self, name: &str) -> &mut ModelUsage {
        self.models.entry(name.to_string()).or_insert_with(|| ModelUsage { name: name.to_string(), ..Default::default() })
    }

    pub fn prompt(&mut self, model: &str, day: &str, n: u64) {
        let (today, week) = (day == self.today, self.week.contains(day));
        *self.prompts_by_day.entry(day.to_string()).or_default() += n;
        let m = self.model(model);
        m.prompts += n;
        if today {
            m.today_prompts += n;
        }
        if week {
            m.week_prompts += n;
        }
    }

    pub fn step(&mut self, model: &str, day: &str, n: u64) {
        let (today, week) = (day == self.today, self.week.contains(day));
        *self.steps_by_day.entry(day.to_string()).or_default() += n;
        let m = self.model(model);
        m.steps += n;
        if today {
            m.today_steps += n;
        }
        if week {
            m.week_steps += n;
        }
    }

    /// Tokens of one model call. The headline count leaves out cache reads (the
    /// context read again on every turn); those are counted as cached.
    pub fn tokens(&mut self, model: &str, day: &str, input: u64, output: u64, cache_read: u64, cache_write: u64) {
        if day == self.today {
            self.today_tokens += input + output + cache_write;
            self.today_cached += cache_read;
        }
        let m = self.model(model);
        m.input_tokens += input;
        m.output_tokens += output;
        m.cached_tokens += cache_read + cache_write;
    }

    pub fn tool(&mut self, name: &str, n: u64) {
        *self.tools.entry(name.to_string()).or_default() += n;
    }

    pub fn is_today(&self, day: &str) -> bool {
        day == self.today
    }

    /// Fill `u`'s day, model, tool and today's counts.
    pub fn finish(self, u: &mut Usage) {
        u.today.prompts = self.prompts_by_day.get(&self.today).copied().unwrap_or(0);
        u.today.steps = self.steps_by_day.get(&self.today).copied().unwrap_or(0);
        u.today.tokens = self.today_tokens;
        u.today.cached = self.today_cached;
        u.totals.prompts = self.prompts_by_day.values().sum();
        u.totals.steps = self.steps_by_day.values().sum();
        u.days = self
            .days
            .iter()
            .map(|d| UsageDay { date: d.clone(), prompts: self.prompts_by_day.get(d).copied().unwrap_or(0), steps: self.steps_by_day.get(d).copied().unwrap_or(0) })
            .collect();
        let mut models: Vec<ModelUsage> = self.models.into_values().filter(|m| m.prompts > 0 || m.steps > 0).collect();
        models.sort_by(|a, b| (b.prompts, b.steps).cmp(&(a.prompts, a.steps)).then(a.name.cmp(&b.name)));
        u.models = models;
        let mut tools: Vec<ToolUsage> = self.tools.into_iter().map(|(name, calls)| ToolUsage { name, calls }).collect();
        tools.sort_by(|a, b| b.calls.cmp(&a.calls).then(a.name.cmp(&b.name)));
        tools.truncate(12);
        u.tools = tools;
    }
}

/// One line of an agent's record as JSON. Tool output can hold invalid UTF-8; the
/// line still counts.
pub fn json_line(line: &[u8]) -> Option<serde_json::Value> {
    match std::str::from_utf8(line) {
        Ok(s) => serde_json::from_str(s).ok(),
        Err(_) => serde_json::from_str(&String::from_utf8_lossy(line)).ok(),
    }
}

/// Untrusted text from a record, on one line.
pub fn text(v: &serde_json::Value, max: usize) -> String {
    crate::sanitize::line(v.as_str().unwrap_or_default(), max)
}
