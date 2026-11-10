import QtQuick
import Quickshell
import Quickshell.Io

// Nyx Agents sessions for a shell widget, from the manager's hub.json.
// Watched, never polled. See docs/HUB.md for the fields.
Item {
  id: root

  readonly property string path: (Quickshell.env("XDG_RUNTIME_DIR") || "/tmp") + "/nyx-agents/hub.json"

  // False until the manager has written its first snapshot.
  property bool running: false
  property int agentCount: 0
  property int blockedCount: 0
  property int doneCount: 0
  property int workingCount: 0
  property int subagentsRunning: 0
  property var sessions: []
  readonly property bool badgeActive: blockedCount > 0 || doneCount > 0 || workingCount > 0

  function clear() {
    running = false
    agentCount = blockedCount = doneCount = workingCount = subagentsRunning = 0
    sessions = []
  }

  function apply(text) {
    try {
      const d = JSON.parse(text)
      if (d.version !== 1) { clear(); return }
      running = true
      agentCount = d.agents || 0
      blockedCount = d.blocked || 0
      doneCount = d.done || 0
      workingCount = d.working || 0
      subagentsRunning = d.subagents_running || 0
      sessions = d.sessions || []
    } catch (e) {
      clear()
    }
  }

  // Show a session in Nyx Agents. `open` is an argv from the manager.
  function open(session) {
    if (session && session.open && session.open.length)
      Quickshell.execDetached(session.open)
  }

  FileView {
    path: root.path
    watchChanges: true
    printErrors: false
    onFileChanged: reload()
    onLoaded: root.apply(text())
    onLoadFailed: root.clear()
  }
}
