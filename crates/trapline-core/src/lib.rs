//! The seam between trapline (events) and its handlers (policy).
//!
//! A handler turns an [`Event`] into a [`Decision`]. `nudge-engine` is the
//! built-in handler; anything that speaks this JSON shape can be another.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    /// Before a wrapped command runs.
    Pre,
    /// After a wrapped command ran (`rc` is set).
    Post,
    /// A command was not found (`tool` is the missing name).
    Notfound,
    /// The working directory changed.
    Chpwd,
    /// A prompt is about to render.
    Prompt,
    /// An agent session is starting (`cwd` is the session directory).
    SessionStart,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    pub phase: Phase,
    #[serde(default)]
    pub tool: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    pub cwd: PathBuf,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub rc: Option<i32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Level {
    Hint,
    Warn,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Message {
    pub level: Level,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Decision {
    /// `false` blocks the command. Defaults to `true` so a handler can reply
    /// with just the fields it cares about.
    #[serde(default = "yes")]
    pub proceed: bool,
    /// argv to run the command under (e.g. `devenv shell --no-tui --`).
    #[serde(default)]
    pub prefix: Vec<String>,
    #[serde(default)]
    pub messages: Vec<Message>,
    /// Env to export; `None` means unset. Lists every key the handler can set,
    /// so the shell can clear stale markers.
    #[serde(default)]
    pub set_env: BTreeMap<String, Option<String>>,
    /// Ids of the rules that fired, for `explain`.
    #[serde(default)]
    pub fired: Vec<String>,
}

fn yes() -> bool {
    true
}

impl Decision {
    /// Fold another decision in: any block blocks, messages and env add up, and
    /// the first non-empty prefix wins.
    pub fn merge(&mut self, o: Decision) {
        self.proceed &= o.proceed;
        if self.prefix.is_empty() {
            self.prefix = o.prefix;
        }
        self.messages.extend(o.messages);
        self.set_env.extend(o.set_env);
        self.fired.extend(o.fired);
    }
}

impl Default for Decision {
    fn default() -> Self {
        Decision {
            proceed: true,
            prefix: vec![],
            messages: vec![],
            set_env: BTreeMap::new(),
            fired: vec![],
        }
    }
}

pub trait Handler {
    fn handle(&self, event: &Event) -> Decision;
}
