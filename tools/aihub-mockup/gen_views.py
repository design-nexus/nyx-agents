#!/usr/bin/env python3
"""Writes the long mockup views (usage, agent-claude, sessions) as plugin view JSON.
The data is sample data shaped like the old hub's usage records and session list."""
import json
import os

HERE = os.path.dirname(os.path.abspath(__file__))


def n(kind, **kw):
    return {"node": kind, **kw}


def hbox(*children, spacing=8, **kw):
    return n("box", orientation="horizontal", spacing=spacing, align="center", children=list(children), **kw)


def vbox(*children, spacing=0, **kw):
    return n("box", orientation="vertical", spacing=spacing, align="fill", children=list(children), **kw)


def label(text, *cls, **kw):
    out = n("label", text=text, **kw)
    if cls:
        out["class"] = list(cls)
    return out


def markup(text, *cls, **kw):
    return label(text, *cls, markup=True, **kw)


def icon(h):
    return f"/plugin/icons/{h}.svg"


def card(*children, spacing=0, cls=()):
    return vbox(*children, spacing=spacing, **{"class": ["card", *cls]})


def section(text):
    return label(text, "section")


def meter(name, left, right, margin, warn=False):
    """A quota window: name, a bar of what's left, when it resets."""
    return hbox(
        label(name, "dim", width=76),
        n("progress", value=left, expand=True, **{"class": ["meter", "warn"] if warn else ["meter"]}),
        label(right, "dim", width=118),
        spacing=10, margin=margin,
    )


def chevron():
    return n("icon", name="go-next-symbolic", size=16, **{"class": ["dim", "trail"]})


# ---------- Usage: every agent on one page ----------

def usage_card(h, name, sub, meters, when, working):
    """An agent's card: a row that opens its page, when it was last used, its quotas."""
    head = [n("row", icon=icon(h), title=name, subtitle=sub, action=f"agent:{h}", expand=True)]
    head.append(label("working", "chip", "ok") if working else label(when.replace(" ", "\u00a0"), "dim"))
    head.append(chevron())
    rows = [hbox(*head, spacing=6)]
    for i, (mname, left, right, warn) in enumerate(meters):
        last = i == len(meters) - 1
        rows.append(meter(mname, left, right, [0, 12, 10 if last else 2, 36], warn))
    return card(*rows, spacing=4)


# (minutes since last used, card). Agents without quotas have only their local records.
agents = [
    (0, usage_card("claude", "Claude Code", "Max · Opus 5.5 · 38 prompts · 1.1 M tokens today", [
        ("5 hours", 0.96, "96% left · 7:40 PM", False),
        ("Week", 0.52, "52% left · Fri", False),
    ], "now", True)),
    (2, usage_card("codex", "Codex", "Pro · GPT-5.5 · 61 prompts · 840 k tokens today", [
        ("5 hours", 0.88, "88% left · 4:55 AM", False),
        ("Week", 0.37, "37% left · Thu", False),
    ], "2 min ago", True)),
    (3, usage_card("opencode", "OpenCode", "No quota · Qwen3-8B (local) · 18 prompts today", [], "3 min ago", False)),
    (6, usage_card("grok", "xAI: Grok and Cursor", "SuperGrok · shared quota · 52 prompts today", [
        ("Month", 0.73, "73% left · Dec 1", False),
        ("Cursor API", 0.12, "12% left · Dec 1", True),
    ], "6 min ago", False)),
    (70, usage_card("hermes", "Hermes", "No quota · 4 prompts today", [], "1 h ago", False)),
    (150, usage_card("antigravity", "Antigravity", "Pro · Gemini 3 · 9 prompts today", [
        ("Week", 0.64, "64% left · Mon", False),
    ], "2 h ago", False)),
    (310, usage_card("muse", "Muse", "No quota · 4 prompts today", [], "5 h ago", False)),
    (2900, usage_card("omp", "oh-my-pi", "No quota · no prompts today", [], "Sun", False)),
]

usage = vbox(
    hbox(
        markup("<b>Today</b>  186 prompts · 2.4 M tokens · 7 agents", expand=True),
        n("button", icon="view-refresh-symbolic", action="refresh", tooltip="Refresh"),
    ),
    *[c for _, c in sorted(agents, key=lambda a: a[0])],
    label("Last used first · quotas for the providers you turned on · updated 1 min ago", "foot"),
    spacing=8,
)


# ---------- An agent's page ----------

def stat(value, name):
    return n("box", orientation="vertical", align="center", expand=True, children=[label(value, "stat"), label(name, "dim", "small")])


def window(name, resets, left, note, warn=False):
    return vbox(
        hbox(label(name, expand=True), label(resets, "dim"), markup(f"<b>{left}</b>")),
        n("progress", value=float(left.split("%")[0]) / 100, **{"class": ["meter", "warn"] if warn else ["meter"]}, margin=[4, 0, 3, 0]),
        label(note, "dim", "small"),
        margin=[6, 12, 6, 12],
    )


def model(name, counts, share):
    return vbox(
        hbox(label(name, expand=True), label(counts, "dim"), markup(f"<b>{share}%</b>")),
        n("progress", value=share / 100, **{"class": ["meter"]}, margin=[4, 0, 0, 0]),
        margin=[4, 12, 6, 12],
    )


def bar_row(name, value, top, right, width=70):
    return hbox(
        label(name, "dim", width=width),
        n("progress", value=value / top, expand=True, **{"class": ["meter"]}),
        label(right, width=86),
        spacing=10, margin=[2, 12, 2, 12],
    )


def card_head(title, *extra):
    return hbox(markup(f"<b>{title}</b>", expand=True), *extra, margin=[8, 12, 4, 12])


def recent(title, sub, sid):
    return hbox(
        n("row", icon=icon("claude"), title=title, subtitle=sub, action=f"resume:{sid}", expand=True),
        n("button", icon="utilities-terminal-symbolic", action=f"terminal:{sid}", tooltip="Resume in a terminal", **{"class": ["trail"]}),
        spacing=4,
    )


days = [("Today", 38), ("Mon 11/09", 63), ("Sun 11/08", 17), ("Sat 11/07", 0), ("Fri 11/06", 41), ("Thu 11/05", 22), ("Wed 11/04", 9)]
tools = [("Bash", 214), ("Read", 160), ("Edit", 98), ("Grep", 61), ("Agent", 12), ("WebFetch", 4)]

agent = vbox(
    hbox(
        n("button", icon="go-previous-symbolic", label="Usage", action="tab:usage", **{"class": ["back"]}),
        label("", expand=True),
        n("button", icon="list-add-symbolic", action="new:claude", tooltip="New Claude Code session"),
        n("button", icon="view-refresh-symbolic", action="refresh:claude", tooltip="Refresh"),
        spacing=4,
    ),
    hbox(
        n("icon", name=icon("claude"), size=32),
        vbox(
            hbox(markup("<b>Claude Code</b>", "agent-name"), label("Working", "chip", "ok"), spacing=8),
            label("Claude Opus 5.5 · Max plan · updated just now", "dim"),
        ),
        spacing=12, margin=[0, 4, 4, 4],
    ),
    card(
        card_head("Today"),
        hbox(stat("38", "prompts"), stat("543", "steps"), stat("1.1 B", "tokens"), stat("912 M", "cached"), stat("118", "all prompts"), spacing=0, margin=[0, 6, 10, 6]),
    ),
    card(
        card_head("Quotas", label("Max", "dim")),
        window("5-hour window", "resets 7:40 PM · in 4 h 38 min", "96% left", "Paced to reset"),
        window("Weekly", "resets Fri 4:00 AM · in 2 d 12 h", "52% left", "Burns 0.5% an hour · on pace · about 22% left at reset"),
        window("Weekly, Opus", "resets Fri 4:00 AM", "71% left", "Burns 0.3% an hour · on pace"),
        n("box", orientation="vertical", height=4),
    ),
    card(
        card_head("Models", hbox(
            n("button", label="Today", action="models:today", **{"class": ["tab", "on"]}),
            n("button", label="7 days", action="models:week", **{"class": ["tab"]}),
            n("button", label="All time", action="models:all", **{"class": ["tab"]}),
            spacing=2, **{"class": ["tabs", "small"]},
        )),
        model("Claude Opus 5.5", "38 prompts · 526 steps", 100),
        model("Claude Sonnet 5", "0 prompts · 17 steps (subagents)", 0),
        n("box", orientation="vertical", height=4),
    ),
    card(
        card_head("Last 7 days"),
        *[bar_row(d, v, 63, f"{v} prompts") for d, v in days],
        n("box", orientation="vertical", height=8),
    ),
    card(
        card_head("Top tools", label("today", "dim")),
        *[bar_row(t, v, 214, f"{v} calls") for t, v in tools],
        n("box", orientation="vertical", height=8),
    ),
    card(
        card_head("Recent sessions"),
        recent("Inspector polish", "nyx-agents · 14 min ago · 526 steps · running", "c41d"),
        recent("Agent workspaces API", "nyx-hyprland · yesterday 21:40 · 1,204 steps", "a9e0"),
        recent("Theme token audit", "nyx-design · yesterday 17:02 · 318 steps", "77b1"),
        recent("Glimpse plugin URLs", "nyx-glimpse · Sun · 211 steps", "5c3e"),
        recent("Bench the renderer", "nyx-agents · Sat · 940 steps", "0f12"),
        hbox(n("button", label="All Claude Code sessions", action="sessions:claude", **{"class": ["flat"]}), margin=[0, 6, 6, 6]),
    ),
    spacing=8,
)


# ---------- Sessions: every past session ----------

def filt(name, on=False):
    return n("button", label=name, action=f"filter:{name.lower()}", **{"class": ["tab", "on"] if on else ["tab"]})


def sess(sid, h, title, sub):
    return n("row", id=sid, icon=icon(h), title=title, subtitle=sub, action=f"pick:{sid}")


def actions(sid, where):
    return vbox(
        hbox(
            n("button", icon="media-playback-start-symbolic", label="Resume", action=f"resume:{sid}", **{"class": ["suggested-action"]}),
            n("button", icon="utilities-terminal-symbolic", label="Terminal", action=f"terminal:{sid}"),
            label("", expand=True),
            n("button", icon="view-reveal-symbolic", action=f"peek:{sid}", tooltip="Peek at the transcript"),
            n("button", icon="folder-open-symbolic", action=f"folder:{sid}", tooltip="Open the folder"),
            n("button", icon="edit-copy-symbolic", action=f"copy:{sid}", tooltip="Copy the session ID"),
            n("button", icon="user-trash-symbolic", action=f"trash:{sid}", tooltip="Move to the Trash (click twice)", **{"class": ["danger"]}),
            spacing=4,
        ),
        label(where, "dim", "small", margin=[6, 2, 0, 2]),
        id=f"{sid}-actions", visible=False, margin=[2, 10, 8, 36], **{"class": ["actions"]},
    )


sessions = vbox(
    hbox(filt("All", True), filt("Claude"), filt("Codex"), filt("Grok"), filt("Cursor"), filt("OpenCode"),
         n("button", icon="pan-down-symbolic", action="filter:more", tooltip="More agents", **{"class": ["tab"]}),
         spacing=2, **{"class": ["tabs"]}),
    n("entry", placeholder="Search titles and projects"),
    section("TODAY"),
    card(
        sess("s1", "claude", "Inspector polish", "nyx-agents · 14 min ago · running in Nyx Agents"),
        sess("s2", "codex", "Fix the CSV parser", "nyx-files · 2 min ago · waiting for you"),
        sess("s3", "opencode", "Update the plugin docs", "nyx-bar · finished 3 min ago"),
        actions("s3", "~/Projects/nyx-bar · ses_5b1e0c77a2 · Qwen3-8B (local) · 42 steps"),
        sess("s4", "grok", "Landing page copy", "website · 6 min ago"),
    ),
    section("YESTERDAY"),
    card(
        sess("s5", "claude", "Agent workspaces API", "nyx-hyprland · 21:40"),
        sess("s6", "cursor", "Theme token audit", "nyx-design · 17:02"),
        sess("s7", "hermes", "Release notes draft", "nyx-packages · 15:30"),
        sess("s8", "codex", "Rollout parser tests", "nyx-agents · 11:12"),
    ),
    section("EARLIER"),
    card(
        sess("s9", "omp", "Box-drawing sprites", "nyx-agents · Sun"),
        sess("s10", "antigravity", "Weather radar tiles", "nyx-weather · Sat"),
        sess("s11", "muse", "Playlist import", "nyx-music · Fri"),
    ),
    hbox(label("214 sessions from 9 agents", "foot", expand=True), n("button", icon="view-refresh-symbolic", label="Rescan", action="rescan")),
    spacing=8,
)

for name, view in (("usage", usage), ("agent-claude", agent), ("sessions", sessions)):
    with open(os.path.join(HERE, "views", f"{name}.json"), "w") as f:
        json.dump(view, f, indent=1, ensure_ascii=False)
        f.write("\n")
