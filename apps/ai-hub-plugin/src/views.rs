//! The hub's views, built from what the manager and AI Core report. Layout
//! follows the approved mockup (docs/mockups, tools/aihub-mockup).

use nyx_agents_proto::{Activity, BlockedOn, Lifecycle, Pace, PastSession, QuotaWindow, Session, Usage};
use nyx_plugin_sdk::protocol::view::Linear;
use nyx_plugin_sdk::protocol::{Draw, Node, Paint};
use nyx_plugin_sdk::view::*;

use crate::{Host, Range, State, Tab};

// ---------- Small pieces ----------

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Short labels keep their words together, so a wrapping popover label doesn't
/// shrink them to their longest word.
fn nb(s: &str) -> String {
    s.replace(' ', "\u{a0}")
}

/// One transcript line, short enough for a popover row.
pub fn nb_trim(s: &str) -> String {
    let s = s.trim_end();
    if s.chars().count() > 90 { format!("{}…", s.chars().take(90).collect::<String>()) } else { s.to_string() }
}

pub fn ago(secs: u64) -> String {
    let d = now().saturating_sub(secs);
    match d {
        0..=59 => "now".into(),
        60..=3599 => format!("{} min ago", d / 60),
        3600..=86399 => format!("{} h ago", d / 3600),
        86400..=172799 => "yesterday".into(),
        _ => format!("{} days ago", d / 86400),
    }
}

fn running_for(ms: u64) -> String {
    let d = now().saturating_sub(ms / 1000);
    match d {
        0..=59 => "just now".into(),
        60..=3599 => format!("{} min", d / 60),
        _ => format!("{} h {} min", d / 3600, (d % 3600) / 60),
    }
}

fn count(n: u64) -> String {
    match n {
        0..=9_999 => n.to_string(),
        10_000..=999_999 => format!("{:.0} k", n as f64 / 1e3),
        1_000_000..=999_999_999 => format!("{:.1} M", n as f64 / 1e6),
        _ => format!("{:.1} B", n as f64 / 1e9),
    }
}

fn gb(bytes: u64) -> String {
    format!("{:.1} GB", bytes as f64 / 1e9)
}

fn local_tm(secs: i64) -> libc::tm {
    let t = secs as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&t, &mut tm) };
    tm
}

fn clock(secs: u64) -> String {
    let tm = local_tm(secs as i64);
    let (h, m) = (tm.tm_hour, tm.tm_min);
    format!("{}:{m:02} {}", if h % 12 == 0 { 12 } else { h % 12 }, if h < 12 { "AM" } else { "PM" })
}

const WEEKDAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];

fn weekday(secs: u64) -> &'static str {
    WEEKDAYS[local_tm(secs as i64).tm_wday.clamp(0, 6) as usize]
}

/// When a window resets, briefly: a time today, else a weekday or a date.
fn resets(at: u64) -> String {
    let left = at.saturating_sub(now());
    if left < 18 * 3600 {
        clock(at)
    } else if left < 6 * 86400 {
        weekday(at).to_string()
    } else {
        let tm = local_tm(at as i64);
        const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
        format!("{} {}", MONTHS[tm.tm_mon.clamp(0, 11) as usize], tm.tm_mday)
    }
}

fn in_time(at: u64) -> String {
    let d = at.saturating_sub(now());
    match d {
        0..=3599 => format!("in {} min", d / 60),
        3600..=86399 => format!("in {} h {} min", d / 3600, (d % 3600) / 60),
        _ => format!("in {} d {} h", d / 86400, (d % 86400) / 3600),
    }
}

/// `YYYY-MM-DD` as "Today" or "Mon 11/09".
fn day_label(date: &str, today: &str) -> String {
    if date == today {
        return "Today".into();
    }
    let p: Vec<i32> = date.split('-').filter_map(|x| x.parse().ok()).collect();
    if p.len() != 3 {
        return date.to_string();
    }
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    tm.tm_year = p[0] - 1900;
    tm.tm_mon = p[1] - 1;
    tm.tm_mday = p[2];
    tm.tm_hour = 12;
    tm.tm_isdst = -1;
    let t = unsafe { libc::mktime(&mut tm) };
    format!("{} {:02}/{:02}", weekday(t as u64), p[1], p[2])
}

fn folder(cwd: &str) -> String {
    std::path::Path::new(cwd).file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default()
}

const KNOWN_ICONS: &[&str] = &["claude", "codex", "grok", "cursor", "opencode", "hermes", "antigravity", "muse", "omp", "crush"];

fn harness_icon(h: &str) -> String {
    if KNOWN_ICONS.contains(&h) { format!("/plugin/icons/{h}.svg") } else { "utilities-terminal-symbolic".into() }
}

fn chip(text: &str, kind: &str) -> Node {
    let n = label(nb(text)).class("chip");
    if kind.is_empty() { n } else { n.class(kind) }
}

fn section(text: &str) -> Node {
    label(text).class("section")
}

fn card(children: impl IntoIterator<Item = Node>) -> Node {
    vbox(children).class("card")
}

fn fill(n: Node) -> Node {
    use nyx_plugin_sdk::protocol::{Align, NodeKind};
    let mut n = n;
    if let NodeKind::Box { align, .. } = &mut n.kind {
        *align = Align::Fill;
    }
    n
}

fn dim(text: impl Into<String>) -> Node {
    label(text).class("dim")
}

fn chevron() -> Node {
    icon("go-next-symbolic").class("dim").class("trail")
}

fn sparkle(size: i32) -> Node {
    let grad = Paint::Linear { linear: Linear { x1: 2.0, y1: 3.0, x2: 20.0, y2: 21.0, stops: vec![(0.0, "accent".into()), (1.0, "success".into())] } };
    vector(
        [0.0, 0.0, 24.0, 24.0],
        vec![
            Draw::Path { d: "M11 3C11.6 8.4 14.6 11.4 20 12C14.6 12.6 11.6 15.6 11 21C10.4 15.6 7.4 12.6 2 12C7.4 11.4 10.4 8.4 11 3Z".into(), fill: Some(grad), stroke: None, width: None },
            Draw::Path {
                d: "M19.5 1.5C19.75 3.1 20.9 4.25 22.5 4.5C20.9 4.75 19.75 5.9 19.5 7.5C19.25 5.9 18.1 4.75 16.5 4.5C18.1 4.25 19.25 3.1 19.5 1.5Z".into(),
                fill: Some(color("accent")),
                stroke: None,
                width: None,
            },
        ],
    )
    .size(size, size)
}

fn dot(token: &str) -> Node {
    vector([0.0, 0.0, 8.0, 8.0], vec![Draw::Circle { cx: 4.0, cy: 4.0, r: 3.5, fill: Some(color(token)), stroke: None, width: None }]).size(8, 8).margin(0, 0, 0, 12)
}

fn row_with(icon: &str, title: &str, subtitle: String, action: &str, trail: Vec<Node>) -> Node {
    let mut c = vec![row(Some(icon), title, Some(subtitle), Some(action)).expand()];
    c.extend(trail);
    hbox(c).spacing(6)
}

// ---------- Shared frame ----------

pub fn name_of(st: &State, harness: &str) -> String {
    st.harnesses.iter().find(|h| h.id == harness).map(|h| h.name.clone()).unwrap_or_else(|| harness.to_string())
}

fn live(st: &State) -> Vec<&Session> {
    let mut v: Vec<&Session> = st.sessions.iter().filter(|s| matches!(s.lifecycle, Lifecycle::Running | Lifecycle::Starting) && s.harness != "shell").collect();
    v.sort_by_key(|s| std::cmp::Reverse(s.status.since.max(s.created)));
    v
}

fn needs_you(s: &Session) -> bool {
    matches!(s.status.activity, Activity::Blocked | Activity::Error)
}

fn subagents_running(s: &Session) -> usize {
    use nyx_agents_proto::SubagentState as S;
    s.subagents.iter().filter(|a| matches!(a.state, S::Running | S::Waiting)).count()
}

pub fn summary(st: &State) -> String {
    let l = live(st);
    if !st.connected {
        return "Nyx Agents isn't running".into();
    }
    if l.is_empty() {
        return match st.usage.iter().filter_map(|u| u.last_used.map(|t| (t, u))).max_by_key(|(t, _)| *t) {
            Some((t, u)) => format!("No agents running · {} used {}", u.name, ago(t)),
            None => "No agents running".into(),
        };
    }
    let needs = l.iter().filter(|s| needs_you(s)).count();
    let subs: usize = l.iter().map(|s| subagents_running(s)).sum();
    let mut parts = vec![format!("{} agent{}", l.len(), if l.len() == 1 { "" } else { "s" })];
    if needs > 0 {
        parts.push(format!("{needs} need{} you", if needs == 1 { "s" } else { "" }));
    }
    if subs > 0 {
        parts.push(format!("{subs} subagent{}", if subs == 1 { "" } else { "s" }));
    }
    parts.join(" · ")
}

/// The popover (and Control Center detail view): header, tabs, then the page.
pub fn popover(st: &State) -> Node {
    let mut top = vec![];
    if st.host == Host::Bar {
        top.push(
            hbox([
                sparkle(28),
                fill(vbox([label("AI Hub").class("title"), dim(summary(st))])).expand(),
                button("open-app", None, Some("list-add-symbolic")).tooltip("New session in Nyx Agents"),
                button("open-app", Some("Agents"), Some("utilities-terminal-symbolic")),
            ])
            .spacing(10),
        );
    }
    let tab = |t: Tab, name: &str, action: &str| {
        let b = button(action, Some(name), None).class("tab").expand();
        if st.tab == t { b.class("on") } else { b }
    };
    top.push(
        hbox([tab(Tab::Agents, "Agents", "tab:agents"), tab(Tab::Usage, "Usage", "tab:usage"), tab(Tab::Sessions, "Sessions", "tab:sessions"), tab(Tab::Local, "Local AI", "tab:local")])
            .spacing(2)
            .class("tabs"),
    );
    if let Some(msg) = &st.notice {
        top.push(label(msg.clone()).class("notice"));
    }
    let page = match (st.tab, &st.agent) {
        (Tab::Usage, Some(h)) => agent_page(st, h),
        (Tab::Usage, None) => usage_tab(st),
        (Tab::Agents, _) => agents_tab(st),
        (Tab::Sessions, _) => sessions_tab(st),
        (Tab::Local, _) => local_tab(st),
    };
    let max = if st.host == Host::Bar { 520 } else { 600 };
    top.push(scroll(max, fill(page.spacing(8))));
    let root = fill(vbox(top).spacing(8));
    if st.host == Host::Bar { root.width(440) } else { root }
}

fn offline(st: &State) -> Option<Node> {
    (!st.connected).then(|| {
        card([
            label("Nyx Agents isn't running").class("tile-title").margin(10, 12, 2, 12),
            dim("Opening Agents starts it.").margin(0, 12, 6, 12),
            hbox([button("open-app", Some("Open Agents"), None)]).margin(0, 12, 10, 12),
        ])
    })
}

// ---------- Agents ----------

fn blocked_chip(s: &Session) -> Node {
    match (s.status.activity, s.status.blocked_on) {
        (Activity::Error, _) => chip("error", "bad"),
        (_, Some(BlockedOn::Question)) => chip("question", "bad"),
        (_, Some(BlockedOn::Auth)) => chip("sign-in", "bad"),
        _ => chip("approval", "bad"),
    }
}

fn session_subtitle(st: &State, s: &Session, when: String) -> String {
    let mut parts = vec![name_of(st, &s.harness), folder(&s.cwd)];
    if let Some(b) = s.worktree.as_ref().and_then(|w| w.branch.clone()) {
        parts.push(b);
    }
    if s.status.turn > 0 && s.status.activity == Activity::Working {
        parts.push(format!("turn {}", s.status.turn + 1));
    }
    parts.push(when);
    parts.retain(|p| !p.is_empty());
    parts.join(" · ")
}

fn agents_tab(st: &State) -> Node {
    if let Some(n) = offline(st) {
        return fill(vbox([n]));
    }
    let l = live(st);
    let mut out = vec![];
    let needs: Vec<&&Session> = l.iter().filter(|s| needs_you(s)).collect();
    if !needs.is_empty() {
        out.push(section("NEEDS YOU"));
        for s in needs {
            let mut c = vec![row_with(&harness_icon(&s.harness), &s.title, session_subtitle(st, s, running_for(s.status.since)), &format!("open:{}", s.id), vec![blocked_chip(s).class("trail")])];
            if let Some(m) = &s.status.message {
                c.push(label(m.clone()).class("ask"));
            }
            c.push(hbox([button(&format!("open:{}", s.id), Some("Open"), None).class("suggested-action"), button(&format!("stop:{}", s.id), Some("Stop"), None)]).spacing(6).margin(0, 10, 8, 36));
            out.push(card(c).class("attention"));
        }
    }
    let working: Vec<&&Session> = l.iter().filter(|s| !needs_you(s) && matches!(s.status.activity, Activity::Working | Activity::Unknown)).collect();
    if !working.is_empty() {
        out.push(section("WORKING"));
        out.push(card(working.iter().map(|s| {
            let mut trail = vec![];
            let subs = subagents_running(s);
            if subs > 0 {
                trail.push(chip(&format!("{subs} subagent{}", if subs == 1 { "" } else { "s" }), ""));
            }
            trail.push(spinner());
            row_with(&harness_icon(&s.harness), &s.title, session_subtitle(st, s, running_for(s.created)), &format!("open:{}", s.id), vec![hbox(trail).spacing(8).class("trail")])
        })));
    }
    // Waiting at their prompt, then ended ones.
    let mut recent: Vec<Node> = l
        .iter()
        .filter(|s| matches!(s.status.activity, Activity::Done | Activity::Idle))
        .map(|s| {
            let c = if s.status.activity == Activity::Done { chip("done", "ok") } else { chip("waiting", "") };
            row_with(&harness_icon(&s.harness), &s.title, session_subtitle(st, s, format!("since {}", ago(s.status.since / 1000))), &format!("open:{}", s.id), vec![c.class("trail")])
        })
        .collect();
    let mut ended: Vec<&Session> = st.sessions.iter().filter(|s| matches!(s.lifecycle, Lifecycle::Exited | Lifecycle::Lost) && s.harness != "shell").collect();
    ended.sort_by_key(|s| std::cmp::Reverse(s.ended.unwrap_or(s.created)));
    for s in ended.into_iter().take(4) {
        let when = format!("ended {}", ago(s.ended.unwrap_or(s.created) / 1000));
        let trail = if s.resumable {
            button(&format!("resume:{}", s.id), None, Some("media-playback-start-symbolic")).tooltip("Resume").class("trail")
        } else {
            chip("ended", "").class("trail")
        };
        recent.push(row_with(&harness_icon(&s.harness), &s.title, session_subtitle(st, s, when), &format!("open:{}", s.id), vec![trail]));
    }
    if !recent.is_empty() {
        out.push(section("RECENT"));
        out.push(card(recent));
    }
    if out.is_empty() {
        out.push(card([
            label("No agents yet").class("tile-title").margin(10, 12, 2, 12),
            dim("Start one in Nyx Agents.").margin(0, 12, 0, 12),
            dim("Agents that need you show here first.").margin(2, 12, 6, 12),
            hbox([button("open-app", Some("Open Agents"), None)]).margin(0, 12, 10, 12),
        ]));
    }
    out.push(separator());
    out.push(footer(st));
    fill(vbox(out))
}

/// Quota meters for the agents closest to their limits, and AI Core's state.
fn footer(st: &State) -> Node {
    let mut c = vec![];
    let mut with_quota: Vec<(&Usage, &QuotaWindow)> =
        st.usage.iter().filter_map(|u| u.quota.windows.iter().max_by(|a, b| a.used_percent.total_cmp(&b.used_percent)).map(|w| (u, w))).collect();
    with_quota.sort_by(|a, b| b.1.used_percent.total_cmp(&a.1.used_percent));
    for (u, w) in with_quota.into_iter().take(2) {
        let short = u.name.split(' ').next().unwrap_or(&u.name);
        c.push(label(nb(&format!("{short} {}", w.label))).class("foot"));
        c.push(progress(1.0 - w.used_percent / 100.0).width(56).class("meter"));
    }
    if c.is_empty() {
        let prompts: u64 = st.usage.iter().map(|u| u.today.prompts).sum();
        c.push(label(format!("{prompts} prompts today")).class("foot"));
    }
    c.push(label("").expand());
    let ai = match &st.local {
        Some(l) if l.loaded > 0 => format!("AI Core: {} loaded", l.loaded),
        Some(_) => "AI Core idle".into(),
        None if st.local_checked => "AI Core off".into(),
        None => String::new(),
    };
    c.push(label(nb(&ai)).class("foot"));
    hbox(c).spacing(10)
}

// ---------- Usage ----------

fn meter(w: &QuotaWindow, margin: [i32; 4]) -> Node {
    let left = (100.0 - w.used_percent).max(0.0);
    let when = w.resets_at.map(|t| format!(" · {}", resets(t))).unwrap_or_default();
    let bar = progress(left / 100.0).expand().class("meter");
    hbox([label(nb(&w.label)).class("dim").width(76), if left < 25.0 { bar.class("warn") } else { bar }, label(nb(&format!("{left:.0}% left{when}"))).class("dim")])
        .spacing(10)
        .margin(margin[0], margin[1], margin[2], margin[3])
}

fn usage_line(u: &Usage) -> String {
    let mut parts = vec![];
    if let Some(p) = &u.quota.plan {
        parts.push(p.clone());
    }
    if let Some(m) = &u.model {
        parts.push(m.clone());
    }
    parts.push(format!("{} prompts", u.today.prompts));
    if u.today.tokens > 0 {
        parts.push(format!("{} tokens today", count(u.today.tokens)));
    } else {
        let last = parts.pop().unwrap_or_default();
        parts.push(format!("{last} today"));
    }
    parts.join(" · ")
}

fn usage_tab(st: &State) -> Node {
    if let Some(n) = offline(st) {
        return fill(vbox([n]));
    }
    let prompts: u64 = st.usage.iter().map(|u| u.today.prompts).sum();
    let tokens: u64 = st.usage.iter().map(|u| u.today.tokens).sum();
    let mut out = vec![hbox([
        markup(format!("<b>Today</b>  {prompts} prompts · {} tokens · {} agents", count(tokens), st.usage.len())).expand(),
        button("refresh", None, Some("view-refresh-symbolic")).tooltip("Refresh"),
    ])];
    for u in &st.usage {
        // Both kept, one shown, so a change patches instead of redrawing.
        let working = chip("working", "ok").visible(u.working);
        let when = label(nb(&u.last_used.map(ago).unwrap_or_default())).class("dim").visible(!u.working);
        let mut rows = vec![row_with(&harness_icon(&u.harness), &u.name, usage_line(u), &format!("agent:{}", u.harness), vec![working, when, chevron()])];
        let n = u.quota.windows.len();
        for (i, w) in u.quota.windows.iter().enumerate() {
            rows.push(meter(w, [0, 12, if i + 1 == n { 10 } else { 2 }, 36]));
        }
        if let Some(e) = &u.quota.error {
            rows.push(label(e.clone()).class("dim").class("small").margin(0, 12, 8, 36));
        }
        out.push(card(rows).spacing(4));
    }
    if st.usage.is_empty() {
        out.push(card([dim("No agent has records yet.").margin(10, 12, 10, 12)]));
    }
    let off: Vec<String> = st.usage.iter().filter(|u| !u.quota.enabled).map(|u| u.name.split(' ').next().unwrap_or(&u.name).to_string()).collect();
    out.push(label("Last used first").class("foot"));
    if !off.is_empty() {
        out.push(label(format!("Quota checks are off for {}.", off.join(", "))).class("foot"));
        out.push(label("Turn them on with [usage] quotas in config.toml.").class("foot"));
    }
    fill(vbox(out))
}

fn stat(value: String, name: &str) -> Node {
    use nyx_plugin_sdk::protocol::{Align, NodeKind};
    let mut n = vbox([label(value).class("stat"), label(name).class("dim").class("small")]).expand();
    if let NodeKind::Box { align, .. } = &mut n.kind {
        *align = Align::Center;
    }
    n
}

fn card_head(title: &str, extra: Vec<Node>) -> Node {
    let mut c = vec![markup(format!("<b>{}</b>", escape(title))).expand()];
    c.extend(extra);
    hbox(c).margin(8, 12, 4, 12)
}

fn window_block(w: &QuotaWindow) -> Node {
    let left = (100.0 - w.used_percent).max(0.0);
    let reset = w.resets_at.map(|t| format!("resets {} · {}", resets(t), in_time(t))).unwrap_or_default();
    let note = match w.pace {
        Pace::Steady => "Paced to reset".to_string(),
        Pace::OnPace => format!("Burns {:.1}% an hour · on pace · about {:.0}% left at reset", w.burn_per_hour, w.left_at_reset.unwrap_or(0.0)),
        Pace::Tight => format!("Burns {:.1}% an hour · tight · about {:.0}% left at reset", w.burn_per_hour, w.left_at_reset.unwrap_or(0.0)),
        Pace::RunsOut => format!("Burns {:.1}% an hour · runs out {} before it resets", w.burn_per_hour, in_time(now() + w.empty_in.unwrap_or(0))),
        Pace::Stale => "Reset since it was last checked".to_string(),
    };
    let bar = progress(left / 100.0).class("meter").margin(4, 0, 3, 0);
    fill(vbox([
        hbox([label(w.label.clone()).expand(), dim(nb(&reset)), markup(format!("<b>{}</b>", nb(&format!("{left:.0}% left"))))]),
        if left < 25.0 { bar.class("warn") } else { bar },
        label(note).class("dim").class("small"),
    ]))
    .margin(6, 12, 6, 12)
}

fn bar_row(name: String, value: u64, top: u64, right: String) -> Node {
    hbox([label(nb(&name)).class("dim").width(70), progress(if top == 0 { 0.0 } else { value as f64 / top as f64 }).expand().class("meter"), label(nb(&right)).width(86)])
        .spacing(10)
        .margin(2, 12, 2, 12)
}

fn agent_page(st: &State, h: &str) -> Node {
    let Some(u) = st.usage.iter().find(|u| u.harness == h) else { return usage_tab(st) };
    let mut out = vec![hbox([
        button("back", Some("Usage"), Some("go-previous-symbolic")).class("back"),
        label("").expand(),
        button(&format!("new:{h}"), None, Some("list-add-symbolic")).tooltip(&format!("New {} session", u.name)),
        button(&format!("refresh:{h}"), None, Some("view-refresh-symbolic")).tooltip("Refresh, and check the quota now"),
    ])
    .spacing(4)];
    let mut sub = vec![];
    if let Some(m) = &u.model {
        sub.push(m.clone());
    }
    if let Some(p) = &u.quota.plan {
        sub.push(format!("{p} plan"));
    }
    sub.push(format!("updated {}", ago(u.updated)));
    let title = vec![markup(format!("<b>{}</b>", escape(&u.name))).class("agent-name"), chip("Working", "ok").visible(u.working)];
    out.push(hbox([icon_sized(&harness_icon(h), 32), fill(vbox([hbox(title).spacing(8), dim(sub.join(" · "))]))]).spacing(12).margin(0, 4, 4, 4));
    out.push(card([
        card_head("Today", vec![]),
        hbox([
            stat(count(u.today.prompts), "prompts"),
            stat(count(u.today.steps), "steps"),
            stat(count(u.today.tokens), "tokens"),
            stat(count(u.today.cached), "cached"),
            stat(count(u.totals.prompts), "all prompts"),
        ])
        .margin(0, 6, 10, 6),
    ]));
    let mut quota = vec![card_head("Quotas", u.quota.plan.iter().map(|p| dim(p.clone())).collect())];
    if !u.quota.enabled {
        quota.push(dim("Off. To check it with its own sign-in, add").margin(0, 12, 0, 12));
        quota.push(label(format!("[usage] quotas = [\"{h}\"]")).class("transcript").margin(2, 12, 0, 12));
        quota.push(dim("to ~/.config/nyx-agents/config.toml.").margin(2, 12, 10, 12));
    } else {
        quota.extend(u.quota.windows.iter().map(window_block));
        if let Some(e) = &u.quota.error {
            quota.push(dim(e.clone()).margin(0, 12, 6, 12));
        }
        quota.push(vbox([]).height(4));
    }
    out.push(card(quota));
    let range = |r: Range, name: &str, action: &str| {
        let b = button(action, Some(name), None).class("tab");
        if st.range == r { b.class("on") } else { b }
    };
    let mut models = vec![card_head(
        "Models",
        vec![hbox([range(Range::Today, "Today", "range:today"), range(Range::Week, "7 days", "range:week"), range(Range::All, "All time", "range:all")]).spacing(2).class("tabs").class("small")],
    )];
    let pick = |m: &nyx_agents_proto::ModelUsage| match st.range {
        Range::Today => (m.today_prompts, m.today_steps),
        Range::Week => (m.week_prompts, m.week_steps),
        Range::All => (m.prompts, m.steps),
    };
    let total: u64 = u.models.iter().map(|m| pick(m).0).sum();
    for m in u.models.iter().filter(|m| pick(m) != (0, 0)).take(6) {
        let (p, s) = pick(m);
        let share = if total == 0 { 0.0 } else { p as f64 / total as f64 };
        models.push(
            fill(vbox([
                hbox([label(m.name.clone()).expand(), dim(nb(&format!("{p} prompts · {s} steps"))), markup(format!("<b>{:.0}%</b>", share * 100.0))]),
                progress(share).class("meter").margin(4, 0, 0, 0),
            ]))
            .margin(4, 12, 6, 12),
        );
    }
    if models.len() == 1 {
        models.push(dim("Nothing in this range.").margin(0, 12, 8, 12));
    }
    out.push(card(models));
    let top = u.days.iter().map(|d| d.prompts).max().unwrap_or(0);
    let today = u.days.last().map(|d| d.date.clone()).unwrap_or_default();
    let mut days = vec![card_head("Last 7 days", vec![])];
    days.extend(u.days.iter().rev().map(|d| bar_row(day_label(&d.date, &today), d.prompts, top, format!("{} prompts", d.prompts))));
    days.push(vbox([]).height(8));
    out.push(card(days));
    if !u.tools.is_empty() {
        let top = u.tools.first().map_or(0, |t| t.calls);
        let mut tools = vec![card_head("Top tools", vec![])];
        tools.extend(u.tools.iter().take(8).map(|t| bar_row(t.name.clone(), t.calls, top, format!("{} calls", count(t.calls)))));
        tools.push(vbox([]).height(8));
        out.push(card(tools));
    }
    let mut recent = vec![card_head("Recent sessions", vec![])];
    for s in u.sessions.iter().take(6) {
        let mut sub = vec![folder(&s.cwd), ago(s.updated), format!("{} prompts", s.prompts)];
        if s.active {
            sub.push("running".into());
        }
        sub.retain(|p| !p.is_empty());
        let trail = button(&format!("pterm:{h}:{}", s.id), None, Some("utilities-terminal-symbolic")).tooltip("Resume in a terminal").class("trail");
        recent.push(row_with(&harness_icon(h), &s.title, sub.join(" · "), &format!("past:{h}:{}", s.id), vec![trail]));
    }
    if u.sessions.is_empty() {
        recent.push(dim("None yet.").margin(0, 12, 8, 12));
    }
    recent.push(hbox([button(&format!("sessions:{h}"), Some(&format!("All {} sessions", u.name)), None).class("flat")]).margin(0, 6, 6, 6));
    out.push(card(recent));
    fill(vbox(out))
}

// ---------- Sessions ----------

fn day_of(secs: u64) -> (i32, i32, i32) {
    let tm = local_tm(secs as i64);
    (tm.tm_year, tm.tm_mon, tm.tm_mday)
}

fn sessions_tab(st: &State) -> Node {
    if let Some(n) = offline(st) {
        return fill(vbox([n]));
    }
    let mut agents: Vec<&str> = vec![];
    for p in &st.history {
        if !agents.contains(&p.harness.as_str()) {
            agents.push(&p.harness);
        }
    }
    let filt = |id: Option<&str>, name: String| {
        let action = format!("filter:{}", id.unwrap_or("all"));
        let b = button(&action, Some(&name), None).class("tab");
        if st.filter.as_deref() == id { b.class("on") } else { b }
    };
    let mut filters = vec![filt(None, "All".into())];
    for a in agents.iter().take(6) {
        let n = name_of(st, a);
        filters.push(filt(Some(a), n.split(' ').next().unwrap_or(&n).to_string()));
    }
    let mut out = vec![hbox(filters).spacing(2).class("tabs"), entry("query", &st.query, Some("Search titles and folders, then press Enter"))];
    let q = st.query.to_lowercase();
    let shown: Vec<&PastSession> = st
        .history
        .iter()
        .filter(|p| st.filter.as_deref().is_none_or(|f| f == p.harness))
        .filter(|p| q.is_empty() || p.title.to_lowercase().contains(&q) || p.cwd.to_lowercase().contains(&q))
        .take(60)
        .collect();
    let today = day_of(now());
    let yesterday = day_of(now() - 86400);
    let mut group: Option<&str> = None;
    let mut rows: Vec<Node> = vec![];
    let flush = |out: &mut Vec<Node>, rows: &mut Vec<Node>, g: Option<&str>| {
        if let Some(g) = g.filter(|_| !rows.is_empty()) {
            out.push(section(g));
            out.push(card(std::mem::take(rows)));
        }
    };
    for p in &shown {
        let d = day_of(p.updated);
        let g = if d == today { "TODAY" } else if d == yesterday { "YESTERDAY" } else { "EARLIER" };
        if group != Some(g) {
            flush(&mut out, &mut rows, group);
            group = Some(g);
        }
        let mut sub = vec![folder(&p.cwd), if d == today || d == yesterday { clock(p.updated) } else { ago(p.updated) }];
        if p.active {
            sub.push(if p.managed.is_some() { "running in Nyx Agents".into() } else { "running".into() });
        } else if p.managed.is_some() {
            sub.push("in Nyx Agents".into());
        }
        sub.retain(|s| !s.is_empty());
        let key = (p.harness.clone(), p.id.clone());
        let picked = st.picked.as_ref() == Some(&key);
        let r = row(Some(&harness_icon(&p.harness)), &p.title, Some(sub.join(" · ")), Some(&format!("pick:{}:{}", p.harness, p.id)));
        rows.push(if picked { r.class("picked") } else { r });
        if picked {
            rows.push(actions(st, p));
        }
    }
    flush(&mut out, &mut rows, group);
    if shown.is_empty() {
        out.push(card([dim(if st.history.is_empty() { "No past sessions found." } else { "Nothing matches." }).margin(10, 12, 10, 12)]));
    }
    let n_agents = agents.len();
    out.push(hbox([
        label(format!("{} sessions from {n_agents} agent{}", st.history.len(), if n_agents == 1 { "" } else { "s" })).class("foot").expand(),
        button("rescan", Some("Rescan"), Some("view-refresh-symbolic")),
    ]));
    fill(vbox(out))
}

fn actions(st: &State, p: &PastSession) -> Node {
    let k = format!("{}:{}", p.harness, p.id);
    let main = if p.active && p.managed.is_some() {
        button(&format!("past:{k}"), Some("Open"), Some("go-next-symbolic")).class("suggested-action")
    } else {
        button(&format!("past:{k}"), Some("Resume"), Some("media-playback-start-symbolic")).class("suggested-action")
    };
    let mut buttons = vec![main];
    if !p.active {
        buttons.push(button(&format!("pterm:{k}"), Some("Terminal"), Some("utilities-terminal-symbolic")));
    }
    buttons.push(label("").expand());
    buttons.push(button(&format!("peek:{k}"), None, Some("view-reveal-symbolic")).tooltip("Peek at the transcript"));
    if !p.cwd.is_empty() {
        buttons.push(button(&format!("folder:{k}"), None, Some("folder-open-symbolic")).tooltip("Open the folder"));
    }
    buttons.push(button(&format!("copy:{k}"), None, Some("edit-copy-symbolic")).tooltip("Copy the session ID"));
    if p.can_trash {
        let armed = st.armed.as_ref() == Some(&(p.harness.clone(), p.id.clone()));
        buttons.push(if armed {
            button(&format!("trash:{k}"), Some("Move to Trash"), Some("user-trash-symbolic")).class("destructive-action")
        } else {
            button(&format!("trash:{k}"), None, Some("user-trash-symbolic")).tooltip("Move to the Trash (click twice)").class("danger")
        });
    }
    let mut where_ = vec![p.cwd.clone(), p.id.clone()];
    if let Some(m) = &p.model {
        where_.push(m.clone());
    }
    where_.push(format!("{} prompts", p.prompts));
    where_.retain(|s| !s.is_empty());
    let mut c = vec![hbox(buttons).spacing(4), label(where_.join(" · ")).class("dim").class("small").margin(6, 2, 0, 2)];
    if let Some((h, id, text)) = &st.peek
        && h == &p.harness
        && id == &p.id
    {
        let body = match text {
            // Its last lines, in place: a scroll inside the page's scroll gets no height.
            Ok(t) => {
                let lines: Vec<&str> = t.lines().filter(|l| !l.trim().is_empty()).collect();
                let tail = &lines[lines.len().saturating_sub(14)..];
                fill(vbox(tail.iter().map(|l| label(crate::views::nb_trim(l)).class("transcript"))))
            }
            Err(e) => dim(e.clone()),
        };
        c.push(body.class("peek").margin(6, 0, 0, 0));
    }
    fill(vbox(c)).margin(2, 10, 8, 36).class("actions")
}

// ---------- Local AI ----------

fn chip_icon() -> Node {
    let line = |x1: f64, y1: f64, x2: f64, y2: f64| Draw::Line { x1, y1, x2, y2, stroke: Some(color("dim_text")), width: Some(1.4), round: true };
    let mut c = vec![
        Draw::Rect { x: 5.0, y: 5.0, w: 14.0, h: 14.0, radius: 3.0, fill: None, stroke: Some(color("text")), width: Some(1.6) },
        Draw::Rect { x: 9.0, y: 9.0, w: 6.0, h: 6.0, radius: 1.5, fill: Some(color("accent")), stroke: None, width: None },
    ];
    for v in [9.0, 12.0, 15.0] {
        c.extend([line(v, 1.5, v, 5.0), line(v, 19.0, v, 22.5), line(1.5, v, 5.0, v), line(19.0, v, 22.5, v)]);
    }
    vector([0.0, 0.0, 24.0, 24.0], c).size(22, 22)
}

fn local_tab(st: &State) -> Node {
    let Some(l) = &st.local else {
        let msg = if st.local_checked { "Not running. It starts when an app asks." } else { "Asking Nyx AI Core…" };
        return fill(vbox([card([label("Nyx AI Core").class("tile-title").margin(10, 12, 2, 12), dim(msg).margin(0, 12, 10, 12)])]));
    };
    let backend = l.backends.first().map(|b| b.to_uppercase()).unwrap_or_else(|| "CPU".into());
    let mut detail = vec!["llama.cpp".to_string(), backend];
    detail.push(if l.loaded == 0 { "nothing loaded".into() } else { format!("{} loaded", l.loaded) });
    if l.queued > 0 {
        detail.push(format!("{} waiting", l.queued));
    }
    let state = match l.worker.as_str() {
        "ready" => chip("running", "ok"),
        "starting" | "loading" => chip(&l.worker, "warn"),
        s => chip(s, "bad"),
    };
    let used = l.ram_total.saturating_sub(l.ram_available);
    let mut out = vec![card([
        hbox([chip_icon(), fill(vbox([markup("<b>Nyx AI Core</b>"), dim(detail.join(" · "))])).expand(), state]).spacing(10).margin(8, 12, 4, 12),
        hbox([
            label("Memory").class("dim").width(56),
            progress(if l.ram_total == 0 { 0.0 } else { used as f64 / l.ram_total as f64 }).expand().class("meter"),
            dim(nb(&format!("{} of {}", gb(used), gb(l.ram_total)))),
        ])
        .spacing(10)
        .margin(0, 12, if l.vram_total > 0 { 2 } else { 8 }, 40),
    ])];
    if l.vram_total > 0 {
        out.push(label(format!("GPU memory {}", gb(l.vram_total))).class("foot").margin(0, 0, 0, 52));
    }
    out.push(section("MODELS"));
    let mut models = vec![];
    for m in l.models.iter().take(8) {
        let live = matches!(m.state.as_str(), "ready" | "busy");
        let token = match m.state.as_str() {
            "ready" | "busy" => "success",
            "loading" | "downloading" | "validating" => "warning",
            "failed" | "missing" => "danger",
            _ => "muted",
        };
        let mut sub = vec![
            match m.state.as_str() {
                "ready" => "Loaded".to_string(),
                "busy" => "Loaded · busy".to_string(),
                "installed" => "On disk".to_string(),
                s => {
                    let mut c = s.chars();
                    c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
                }
            },
            gb(m.size),
        ];
        sub.push(m.capabilities.iter().filter(|c| *c != "draft").cloned().collect::<Vec<_>>().join(", "));
        if m.update {
            sub.push("update available".into());
        }
        sub.retain(|s| !s.is_empty());
        let action = if live { button(&format!("unload:{}", m.id), Some("Unload"), None) } else { button(&format!("load:{}", m.id), Some("Load"), None) };
        let mut c = vec![dot(token), row(None, &m.name, Some(sub.join(" · ")), Some(&format!("model:{}", m.id))).expand()];
        if matches!(m.state.as_str(), "ready" | "busy" | "installed") {
            c.push(action.class("trail"));
        }
        models.push(hbox(c));
    }
    if l.models.is_empty() {
        models.push(dim("No models yet. nyx-ai pull org/repo:QUANT downloads one.").margin(10, 12, 10, 12));
    }
    if l.models.len() > 8 {
        models.push(dim(format!("{} more: nyx-ai models lists them all", l.models.len() - 8)).margin(4, 12, 8, 12));
    }
    out.push(card(models));
    out.push(section("CODING TOOLS"));
    out.push(card([row(Some("network-server-symbolic"), "OpenAI-compatible API", Some("127.0.0.1:8080/v1 · loopback only · click to copy".into()), Some("copy-endpoint"))]));
    fill(vbox(out))
}

// ---------- Bar item and Control Center tiles ----------

pub fn bar_item(st: &State) -> Node {
    let l = live(st);
    let needs = l.iter().filter(|s| needs_you(s)).count();
    let working = l.iter().filter(|s| s.status.activity == Activity::Working).count();
    hbox([
        sparkle(16),
        label(working.to_string()).id("working").class("dim").visible(working > 0),
        badge(needs.to_string()).id("needs").class("needs").visible(needs > 0),
    ])
    .spacing(5)
    .tooltip(&format!("AI Hub · {}", summary(st)))
}

pub fn compact_status(st: &State) -> (String, bool) {
    let l = live(st);
    let needs = l.iter().filter(|s| needs_you(s)).count();
    let working = l.iter().filter(|s| s.status.activity == Activity::Working).count();
    match (needs, working, l.len()) {
        (n, _, _) if n > 0 => (format!("{n} need{} you", if n == 1 { "s" } else { "" }), true),
        (_, w, _) if w > 0 => (format!("{w} working"), true),
        (_, _, 0) => ("Idle".into(), false),
        (_, _, n) => (format!("{n} waiting"), true),
    }
}

pub fn overview_tile(st: &State) -> Node {
    let l = live(st);
    let needs = l.iter().filter(|s| needs_you(s)).count();
    let (status, _) = compact_status(st);
    let head_chip = if needs > 0 { chip(&status, "bad") } else if l.is_empty() { chip("idle", "") } else { chip(&status, "ok") };
    let mut c = vec![hbox([sparkle(18), label("AI Hub").class("tile-title").expand(), head_chip]).spacing(8)];
    let mut ordered: Vec<&&Session> = l.iter().filter(|s| needs_you(s)).collect();
    ordered.extend(l.iter().filter(|s| !needs_you(s)));
    for s in ordered.into_iter().take(3) {
        let (token, pulse) = if needs_you(s) { ("danger", false) } else if s.status.activity == Activity::Working { ("warning", true) } else { ("success", false) };
        let d = dot(token).margin(0, 5, 0, 5);
        c.push(
            hbox([
                if pulse { d.class("dot") } else { d },
                markup(format!("<b>{}</b>  <span alpha='65%'>{}</span>", escape(&s.title), escape(&name_of(st, &s.harness)))).expand().class("line"),
                label(nb(&running_for(s.created))).class("dim").class("line"),
            ])
            .spacing(8),
        );
    }
    if l.is_empty() {
        c.push(dim(summary(st)).class("line"));
    }
    fill(vbox(c).spacing(5))
}
