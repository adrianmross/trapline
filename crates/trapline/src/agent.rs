//! `trapline agent <name>`: speak an agent's hook protocol and answer with the
//! same rules the shell uses. Agent shells often skip your rc files, so this is
//! how the rules reach them.

use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::PathBuf;
use trapline_core::{Decision, Event, Handler, Phase};

/// Words that run another command; the real tool follows.
const PASSTHROUGH: &[&str] = &["sudo", "command", "exec", "time", "nohup", "env"];

/// Split a shell command line into the simple commands it runs.
///
/// Handles quotes, `&&`, `||`, `;`, `|` and `&`, and skips leading `VAR=x`
/// assignments and wrappers like `sudo`. It does not look inside `$(...)`,
/// subshells or `cd`, so a rule can miss a command hidden there.
pub fn simple_commands(line: &str) -> Vec<(String, Vec<String>)> {
    let Some(words) = shlex::split(line) else {
        return vec![];
    };
    let mut out = vec![];
    let mut cur: Vec<String> = vec![];
    let mut flush = |cur: &mut Vec<String>| {
        let mut it = cur
            .drain(..)
            .skip_while(|w| is_assignment(w) || PASSTHROUGH.contains(&w.as_str()));
        if let Some(tool) = it.next() {
            out.push((tool, it.collect()));
        }
    };
    for w in words {
        // A trailing `;`, `&` or `|` (alone or glued to a word) ends the command.
        let body = w.trim_end_matches([';', '&', '|']);
        if !body.is_empty() {
            cur.push(body.to_string());
        }
        if body.len() != w.len() {
            flush(&mut cur);
        }
    }
    flush(&mut cur);
    out
}

fn is_assignment(w: &str) -> bool {
    w.split_once('=').is_some_and(|(k, _)| {
        !k.is_empty() && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    })
}

/// Events for one hook invocation, or `None` if the hook is not one we handle.
pub fn events(hook: &Value, mut env: BTreeMap<String, String>) -> Option<(String, Vec<Event>)> {
    let name = hook.get("hook_event_name")?.as_str()?.to_string();
    let cwd = PathBuf::from(hook.get("cwd")?.as_str()?);
    // Whoever calls this is an agent, even if no marker variable is set.
    env.entry("AGENT".into()).or_insert_with(|| "1".into());
    let event = |phase, tool: Option<String>, args| Event {
        phase,
        tool,
        args,
        cwd: cwd.clone(),
        env: env.clone(),
        rc: None,
    };
    let evs = match name.as_str() {
        "SessionStart" => vec![event(Phase::SessionStart, None, vec![])],
        "PreToolUse" if hook.get("tool_name")?.as_str()? == "Bash" => {
            let cmd = hook.pointer("/tool_input/command")?.as_str()?;
            simple_commands(cmd)
                .into_iter()
                .map(|(t, a)| event(Phase::Pre, Some(t), a))
                .collect()
        }
        _ => return None,
    };
    Some((name, evs))
}

/// Merge decisions for several commands: any block blocks, messages concatenate.
pub fn merge(decisions: impl IntoIterator<Item = Decision>) -> Decision {
    let mut out = Decision::default();
    for d in decisions {
        out.proceed &= d.proceed;
        out.messages.extend(d.messages);
        out.fired.extend(d.fired);
    }
    out
}

/// The hook's stdout JSON, or `None` when there is nothing to say.
pub fn claude_output(hook_event: &str, d: &Decision) -> Option<Value> {
    let text = d
        .messages
        .iter()
        .map(|m| m.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    if !d.proceed {
        return Some(json!({"hookSpecificOutput": {
            "hookEventName": hook_event,
            "permissionDecision": "deny",
            "permissionDecisionReason": text,
        }}));
    }
    if text.is_empty() {
        return None;
    }
    Some(json!({
        "systemMessage": text,
        "hookSpecificOutput": {"hookEventName": hook_event, "additionalContext": text},
    }))
}

/// Run the hook: stdin JSON in, decision JSON out.
pub fn run_claude(
    stdin: &str,
    handler: &dyn Handler,
    env: BTreeMap<String, String>,
) -> Option<Value> {
    let hook: Value = serde_json::from_str(stdin).ok()?;
    let (name, evs) = events(&hook, env)?;
    let d = merge(evs.iter().map(|e| handler.handle(e)));
    claude_output(&name, &d)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cmds(s: &str) -> Vec<String> {
        simple_commands(s)
            .into_iter()
            .map(|(t, a)| format!("{t} {}", a.join(" ")).trim().to_string())
            .collect()
    }

    #[test]
    fn splits_compound_commands_and_skips_wrappers() {
        assert_eq!(cmds("git commit -m 'a && b'"), ["git commit -m a && b"]);
        assert_eq!(
            cmds("cd x && git push; ls | wc -l"),
            ["cd x", "git push", "ls", "wc -l"]
        );
        assert_eq!(
            cmds("FOO=1 sudo git worktree add ../x"),
            ["git worktree add ../x"]
        );
        assert_eq!(cmds("echo hi&"), ["echo hi"]);
        assert!(cmds("echo 'unterminated").is_empty());
    }

    #[test]
    fn claude_output_shapes() {
        let mut d = Decision::default();
        assert!(claude_output("PreToolUse", &d).is_none());
        d.messages.push(trapline_core::Message {
            level: trapline_core::Level::Warn,
            text: "w".into(),
        });
        let warn = claude_output("PreToolUse", &d).unwrap();
        assert_eq!(warn["hookSpecificOutput"]["additionalContext"], "w");
        assert!(warn["hookSpecificOutput"]
            .get("permissionDecision")
            .is_none());
        d.proceed = false;
        let deny = claude_output("PreToolUse", &d).unwrap();
        assert_eq!(deny["hookSpecificOutput"]["permissionDecision"], "deny");
    }
}
