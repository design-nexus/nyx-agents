//! The AI Hub snapshot: a small JSON file rewritten on every change, so a
//! shell widget can watch it instead of polling anything.

use nyx_agents_proto::{Activity, Hub, HubSession, HubUsage, Lifecycle, Session, SubagentState, Usage, now_ms};

/// An agent's usage in brief, with the quota window closest to running out.
pub fn brief(u: &Usage) -> HubUsage {
    HubUsage {
        harness: u.harness.clone(),
        name: u.name.clone(),
        working: u.working,
        last_used: u.last_used,
        today_prompts: u.today.prompts,
        today_tokens: u.today.tokens,
        quota: u.quota.windows.iter().max_by(|a, b| a.used_percent.total_cmp(&b.used_percent)).cloned(),
    }
}

pub fn build(sessions: &[Session], usage: &[Usage]) -> Hub {
    let live: Vec<&Session> =
        sessions.iter().filter(|s| matches!(s.lifecycle, Lifecycle::Running | Lifecycle::Starting)).collect();
    let count = |a: Activity| live.iter().filter(|s| s.status.activity == a).count();
    let running_subs =
        |s: &Session| s.subagents.iter().filter(|a| matches!(a.state, SubagentState::Running | SubagentState::Waiting)).count();
    Hub {
        version: 1,
        updated: now_ms(),
        agents: live.len(),
        blocked: count(Activity::Blocked),
        done: count(Activity::Done),
        working: count(Activity::Working),
        subagents_running: live.iter().map(|s| running_subs(s)).sum(),
        sessions: live
            .iter()
            .map(|s| HubSession {
                id: s.id.clone(),
                harness: s.harness.clone(),
                title: s.title.clone(),
                project: s.project.clone(),
                cwd: s.cwd.clone(),
                branch: s.worktree.as_ref().and_then(|w| w.branch.clone()),
                activity: s.status.activity,
                blocked_on: s.status.blocked_on,
                turn: s.status.turn,
                started: s.created,
                status_since: s.status.since,
                subagents_running: running_subs(s),
                subagents_total: s.subagents.len(),
                workspace: s.workspace.clone(),
                open: vec!["nyx-agents".into(), "--open".into(), format!("session/{}", s.id)],
            })
            .collect(),
        usage: usage.iter().map(brief).collect(),
    }
}

pub fn write(sessions: &[Session], usage: &[Usage]) -> std::io::Result<()> {
    let dir = crate::paths::runtime_dir();
    crate::paths::private_dir(&dir)?;
    crate::paths::write_private(&crate::paths::hub_file(), &serde_json::to_vec(&build(sessions, usage)).unwrap())
}

#[cfg(test)]
mod tests {
    use super::*;
    use nyx_agents_proto::Status;

    fn session(id: &str, life: Lifecycle, act: Activity) -> Session {
        Session {
            id: id.into(),
            harness: "claude".into(),
            title: "t".into(),
            project: None,
            workspace: None,
            cwd: "/x".into(),
            worktree: None,
            harness_session: Some("secret-looking-id".into()),
            lifecycle: life,
            status: Status { activity: act, ..Default::default() },
            exit_code: None,
            pid: Some(1),
            created: 1,
            ended: None,
            cols: 80,
            rows: 24,
            resumable: false,
            subagents: vec![],
        }
    }

    #[test]
    fn counts_live_sessions_only_and_carries_no_private_fields() {
        let hub = build(
            &[
                session("a", Lifecycle::Running, Activity::Blocked),
                session("b", Lifecycle::Running, Activity::Working),
                session("c", Lifecycle::Exited, Activity::Done),
            ],
            &[],
        );
        assert_eq!((hub.agents, hub.blocked, hub.working, hub.done), (2, 1, 1, 0));
        let json = serde_json::to_string(&hub).unwrap();
        assert!(!json.contains("secret-looking-id"), "harness session ids stay out of the hub");
        assert!(!json.contains("pid"));
        assert_eq!(hub.sessions[0].open, ["nyx-agents", "--open", "session/a"]);
    }
}
