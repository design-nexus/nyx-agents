import QtQuick
import QtCore
import Quickshell
import QtQuick.Controls as Controls
import QtQuick.Layouts
import "nyx/Ui" as Nyx
import "Model.js" as Model
import "nyx/Theme" as Theme

Rectangle {
    id: root
    implicitWidth: 440
    implicitHeight: 740
    color: Nyx.Style.bg
    radius: Nyx.Style.radiusDialog
    property bool active: visible
    signal closeRequested
    property string tab: "Agents"
    property string agentId: ""
    property string sessionId: ""
    property string expandedId: ""
    property string query: ""
    property string sessionAgentFilter: ""
    property var manager: ({})
    property var local: ({})
    property string managerError: ""
    property string localError: ""
    property string notice: ""
    property bool busy: false
    property double refreshed: 0
    property var agentChoices: ({})
    readonly property var discoveredAgents: Model.catalog(manager)
    readonly property var agents: Model.agents(manager, agentChoices)
    readonly property var sessions: Model.sessions(manager).filter(s => agents.some(a => a.id === s.agent))
    readonly property var agent: agents.find(a => a.id === agentId) || null
    readonly property var session: sessions.find(s => s.id === sessionId) || null
    onAgentsChanged: if (sessionAgentFilter !== "" && !agents.some(a => a.id === sessionAgentFilter))
        sessionAgentFilter = ""
    Settings {
        id: preferences
        location: "file://" + (Quickshell.env("XDG_CONFIG_HOME") || (Quickshell.env("HOME") + "/.config")) + "/nyx-ai-lens/settings.ini"
        category: "agents"
    }
    function loadAgentChoices() {
        preferences.sync();
        agentChoices = Model.parseChoices(preferences.value("choices", "{}"));
    }
    function setAgentEnabled(id, on) {
        let next = Object.assign({}, agentChoices);
        next[id] = on;
        agentChoices = next;
        preferences.setValue("choices", JSON.stringify(next));
        preferences.sync();
        if (agentId === id && !on)
            agentId = "";
        if (session && !agents.some(a => a.id === session.agent))
            sessionId = "";
    }
    function resetAgentChoices() {
        agentChoices = {};
        preferences.setValue("choices", "{}");
        preferences.sync();
    }
    Component.onCompleted: loadAgentChoices()
    function swatch(name) {
        return Theme.NyxTheme.qmlColor(Theme.NyxTheme.swatches[name] || Theme.NyxTheme.legacy.accent);
    }
    function tone(id) {
        return root.swatch(({
                codex: "green",
                claude: "coral",
                antigravity: "pink",
                grok: "violet",
                xai: "violet",
                cursor: "blue"
            })[id] || "teal");
    }
    function name(id) {
        let a = agents.find(a => a.id === id);
        return a ? a.name : id;
    }
    function refresh(force) {
        if (!backend.ready) {
            backend.restart();
            return;
        }
        if (busy)
            return;
        busy = true;
        backend.send(force ? "refresh" : "snapshot", {});
    }
    function availableModels() {
        let loaded = Model.array((root.local.resource_status || {}).loaded);
        return Model.array((root.local.list_models || {}).models).filter(m => !loaded.some(l => l.id === m.id));
    }
    function matching() {
        return sessions.filter(s => (sessionAgentFilter === '' || s.agent === sessionAgentFilter) && (s.title + ' ' + s.cwd + ' ' + name(s.agent) + ' ' + (s.source || '')).toLowerCase().indexOf(query.toLowerCase()) >= 0);
    }
    function go(t) {
        tab = t;
        agentId = "";
        sessionId = "";
        scroll.contentItem.contentY = 0;
    }
    onActiveChanged: {
        if (active) {
            loadAgentChoices();
            refresh(false);
        } else
            busy = false;
    }
    Bridge {
        id: backend
        binary: "nyx-ai-lens"
        enabled: root.active
        onReply: (op, result, error, request) => {
            if (op === 'ready') {
                root.busy = false;
                if (root.active)
                    root.refresh(false);
                return;
            }
            root.busy = false;
            if (error) {
                root.notice = error;
                return;
            }
            root.managerError = result.manager_error || "";
            root.localError = result.local_error || "";
            if (result.manager)
                root.manager = result.manager;
            if (result.local)
                root.local = result.local;
            root.refreshed = result.checked || Date.now();
            if (root.sessionId && !root.session)
                root.sessionId = "";
            if (root.agentId && !root.agent)
                root.agentId = "";
        }
    }
    Timer {
        interval: 5000
        repeat: true
        running: root.active
        onTriggered: root.refresh(false)
    }
    ColumnLayout {
        anchors.fill: parent
        anchors.margins: 14
        spacing: 10
        RowLayout {
            Layout.fillWidth: true
            Layout.minimumWidth: root.width - 28
            Plain {
                text: "✧"
                color: Nyx.Style.accent
                role: "title"
            }
            ColumnLayout {
                Layout.fillWidth: true
                spacing: 1
                Plain {
                    text: "AI Lens"
                    strong: true
                }
                Plain {
                    text: root.agents.filter(a => a.attention).length + " needs you · " + root.sessions.filter(s => s.status === 'Active').length + " working"
                    dim: true
                    role: "caption"
                }
            }
            Action {
                text: "⚙"
                toggled: root.tab === "Settings"
                Accessible.name: "Choose agents"
                onClicked: root.go(root.tab === "Settings" ? "Agents" : "Settings")
            }
            Action {
                text: root.busy ? "…" : "↻"
                enabled: !root.busy
                Accessible.name: "Refresh telemetry"
                onClicked: root.refresh(true)
            }
            Action {
                text: "×"
                Accessible.name: "Close AI Lens"
                onClicked: root.closeRequested()
            }
        }
        RowLayout {
            Layout.fillWidth: true
            spacing: 3
            Repeater {
                model: ["Agents", "Sessions", "Local AI"]
                Nyx.Button {
                    required property string modelData
                    Layout.fillWidth: true
                    text: modelData
                    toggled: root.tab === modelData
                    implicitHeight: 30
                    onClicked: root.go(modelData)
                }
            }
        }
        RowLayout {
            visible: root.notice !== ""
            Layout.fillWidth: true
            Notice {
                Layout.fillWidth: true
                text: root.notice
                error: true
            }
            Action {
                text: "×"
                onClicked: root.notice = ""
            }
        }
        Controls.ScrollView {
            id: scroll
            Layout.fillWidth: true
            Layout.fillHeight: true
            clip: true
            contentWidth: availableWidth
            Column {
                width: scroll.availableWidth
                spacing: 10
                Notice {
                    width: parent.width
                    visible: root.tab !== "Local AI" && root.managerError !== ""
                    text: "Agent telemetry disconnected. " + root.managerError
                    error: true
                }
                Column {
                    width: parent.width
                    spacing: 10
                    visible: root.tab === "Settings"
                    Action {
                        text: "‹ Agents"
                        onClicked: root.go("Agents")
                    }
                    Plain {
                        text: "Choose agents"
                        role: "title"
                    }
                    Plain {
                        width: parent.width
                        text: "Only enabled agents appear in cards, sessions and overview counts. Your choices are saved."
                        dim: true
                        role: "caption"
                    }
                    Repeater {
                        model: root.discoveredAgents.slice().sort((a, b) => a.name.localeCompare(b.name))
                        Card {
                            required property var modelData
                            width: parent.width
                            RowLayout {
                                width: parent.width
                                ColumnLayout {
                                    Layout.fillWidth: true
                                    spacing: 4
                                    Plain {
                                        Layout.fillWidth: true
                                        text: modelData.name
                                        strong: true
                                    }
                                    Plain {
                                        Layout.fillWidth: true
                                        text: Model.array(modelData.quota.windows).length ? Model.array(modelData.quota.windows).length + " subscription quota pools" : modelData.usage && modelData.usage.has_records ? "Usage records · no subscription data" : "No usage or subscription data"
                                        dim: true
                                        role: "caption"
                                    }
                                }
                                Nyx.Toggle {
                                    objectName: "agent-toggle-" + modelData.id
                                    checked: Model.enabled(modelData, root.agentChoices)
                                    Accessible.name: modelData.name
                                    onToggled: on => root.setAgentEnabled(modelData.id, on)
                                }
                            }
                        }
                    }
                    Plain {
                        width: parent.width
                        visible: root.discoveredAgents.length === 0
                        text: "Connect Nyx Agents to discover available agents."
                        dim: true
                    }
                    Action {
                        text: "Reset to agents with data"
                        onClicked: root.resetAgentChoices()
                    }
                }
                // AGENTS OVERVIEW
                Column {
                    width: parent.width
                    spacing: 8
                    visible: root.tab === "Agents" && root.agentId === ""
                    Card {
                        objectName: "overview-info"
                        paddingY: 6
                        paddingX: 10
                        width: parent.width
                        Row {
                            width: parent.width
                            Repeater {
                                model: [
                                    {
                                        label: "Needs you",
                                        value: root.agents.filter(a => a.attention).length,
                                        sub: "attention",
                                        color: Nyx.Style.warning
                                    },
                                    {
                                        label: "Working",
                                        value: root.sessions.filter(s => s.status === 'Active').length,
                                        sub: "sessions",
                                        color: root.swatch("teal")
                                    },
                                    {
                                        label: "Today",
                                        value: root.agents.reduce((n, a) => n + Number((a.usage && a.usage.today || {}).prompts || 0), 0),
                                        sub: "prompts",
                                        color: root.swatch("amber")
                                    },
                                    {
                                        label: "Local AI",
                                        value: root.localError ? "Offline" : ((root.local.worker_status || {}).metrics || {}).active > 0 ? "Busy" : root.local.worker_status ? "Ready" : "Unknown",
                                        sub: "worker",
                                        color: root.swatch("blue")
                                    }
                                ]
                                Column {
                                    required property var modelData
                                    width: parent.width / 4
                                    spacing: 1
                                    Plain {
                                        size: 10
                                        wrapMode: Text.NoWrap
                                        text: modelData.label
                                        role: "caption"
                                        dim: true
                                    }
                                    Plain {
                                        text: String(modelData.value)
                                        color: modelData.color
                                        size: 16
                                    }
                                    Plain {
                                        size: 10
                                        wrapMode: Text.NoWrap
                                        text: modelData.sub
                                        role: "caption"
                                        dim: true
                                    }
                                }
                            }
                        }
                    }
                    Plain {
                        text: "Agents · " + root.agents.length + " shown"
                        dim: true
                        role: "caption"
                    }
                    Repeater {
                        model: root.agents
                        Card {
                            id: agentCard
                            objectName: "agent-card-" + modelData.id
                            paddingX: 12
                            paddingY: 10
                            contentSpacing: 5
                            required property var modelData
                            width: parent.width
                            accent: root.tone(modelData.id)
                            interactive: true
                            Accessible.name: modelData.name + ", " + modelData.state
                            onClicked: {
                                if (!modelData.usage && modelData.sessions.length === 0) {
                                    root.expandedId = root.expandedId === modelData.id ? "" : modelData.id;
                                } else {
                                    root.agentId = modelData.id;
                                    scroll.contentItem.contentY = 0;
                                }
                            }
                            Row {
                                width: parent.width
                                Plain {
                                    width: parent.width * .7
                                    text: modelData.name + (modelData.quota.plan ? "  ·  " + modelData.quota.plan : "")
                                    strong: true
                                }
                                Plain {
                                    width: parent.width * .3
                                    text: modelData.state
                                    wrapMode: Text.NoWrap
                                    maximumLineCount: 1
                                    role: "caption"
                                    horizontalAlignment: Text.AlignRight
                                    color: modelData.attention ? Nyx.Style.warning : modelData.working ? root.swatch("teal") : Nyx.Style.dimText
                                }
                            }
                            Plain {
                                width: parent.width
                                text: modelData.current ? modelData.current.title + " · " + Model.shortPath(modelData.current.cwd) : "No session information"
                                dim: true
                                wrapMode: Text.NoWrap
                                maximumLineCount: 1
                                role: "caption"
                            }
                            Plain {
                                width: parent.width
                                visible: !!modelData.usage
                                text: Model.number((modelData.usage && modelData.usage.today || {}).prompts) + " prompts · " + Model.compact((modelData.usage && modelData.usage.today || {}).tokens) + " tokens today"
                                dim: true
                                wrapMode: Text.NoWrap
                                maximumLineCount: 1
                                role: "caption"
                            }
                            Plain {
                                width: parent.width
                                visible: false
                                text: modelData.current && modelData.current.message || "User input requested"
                                color: Nyx.Style.warning
                                wrapMode: Text.NoWrap
                                maximumLineCount: 1
                                role: "caption"
                            }
                            Plain {
                                width: parent.width
                                visible: !modelData.quota.windows || modelData.quota.windows.length === 0
                                text: modelData.quota.available && !modelData.quota.enabled ? "Quota checks disabled" : "Subscription allowance unavailable"
                                dim: true
                                wrapMode: Text.NoWrap
                                maximumLineCount: 1
                                role: "caption"
                            }
                            Repeater {
                                model: Model.array(agentCard.modelData.quota.windows)
                                CompactQuota {
                                    required property var modelData
                                    required property int index
                                    width: parent.width
                                    label: modelData.label
                                    remaining: Model.remaining(modelData)
                                    reset: Model.reset(modelData.resets_at)
                                    tone: root.swatch(["teal", "blue", "violet", "pink", "coral"][index % 5])
                                }
                            }
                            Plain {
                                width: parent.width
                                visible: root.expandedId === modelData.id
                                text: "No detailed records received for this agent."
                                dim: true
                                wrapMode: Text.NoWrap
                                maximumLineCount: 1
                                role: "caption"
                            }
                            Plain {
                                width: parent.width
                                visible: !!modelData.quota.error
                                text: "Quota refresh failed · last known allowance"
                                color: Nyx.Style.warning
                                wrapMode: Text.NoWrap
                                maximumLineCount: 1
                                role: "caption"
                            }
                        }
                    }
                    Plain {
                        width: parent.width
                        visible: root.agents.length === 0
                        text: root.discoveredAgents.length ? "No agents enabled. Use ⚙ to choose agents." : root.managerError ? "Connect Nyx Agents to discover supported agents." : "No supported agents discovered."
                        dim: true
                    }
                }
                // AGENT DETAIL
                Column {
                    width: parent.width
                    spacing: 10
                    visible: root.tab === "Agents" && root.agent !== null
                    Action {
                        text: "‹ Agents"
                        onClicked: root.agentId = ""
                    }
                    Plain {
                        text: root.agent ? root.agent.name : ""
                        role: "title"
                    }
                    Plain {
                        text: root.agent ? root.agent.state : ""
                        color: root.agent && root.agent.attention ? Nyx.Style.warning : Nyx.Style.dimText
                    }
                    Plain {
                        text: "Subscription allowance"
                        role: "caption"
                        dim: true
                    }
                    Repeater {
                        model: root.agent ? Model.array(root.agent.quota.windows) : []
                        Card {
                            required property var modelData
                            required property int index
                            width: parent.width
                            Quota {
                                width: parent.width
                                label: modelData.label
                                remaining: Model.remaining(modelData)
                                reset: Model.reset(modelData.resets_at)
                                tone: root.swatch(["teal", "blue", "violet", "pink"][index % 4])
                            }
                        }
                    }
                    Notice {
                        width: parent.width
                        visible: !!root.agent && !!root.agent.quota.error
                        text: root.agent ? "Last known quota · " + (root.agent.quota.error || "") : ""
                    }
                    Plain {
                        width: parent.width
                        visible: root.agent && Model.array(root.agent.quota.windows).length === 0
                        text: "Subscription allowance unavailable"
                        dim: true
                    }
                    Card {
                        width: parent.width
                        visible: !!root.agent && !!root.agent.usage
                        Plain {
                            text: "Today · local records"
                            strong: true
                        }
                        Repeater {
                            model: root.agent && root.agent.usage ? Object.keys(root.agent.usage.today || {}) : []
                            Pair {
                                required property string modelData
                                width: parent.width
                                label: modelData
                                value: root.agent ? Model.number(root.agent.usage.today[modelData]) : ""
                            }
                        }
                    }
                    Card {
                        width: parent.width
                        visible: root.agent && Model.array((root.agent.usage || {}).days).length > 0
                        Plain {
                            text: "Last 7 days · prompts"
                            strong: true
                        }
                        Row {
                            width: parent.width
                            spacing: 5
                            Repeater {
                                model: root.agent ? Model.array((root.agent.usage || {}).days).slice(-7) : []
                                Column {
                                    required property var modelData
                                    required property int index
                                    width: (parent.width - 30) / 7
                                    spacing: 5
                                    Plain {
                                        width: parent.width
                                        text: Model.number(modelData.prompts)
                                        role: "caption"
                                        horizontalAlignment: Text.AlignHCenter
                                    }
                                    Item {
                                        width: parent.width
                                        height: 54
                                        Rectangle {
                                            anchors.bottom: parent.bottom
                                            anchors.horizontalCenter: parent.horizontalCenter
                                            width: Math.min(24, parent.width)
                                            height: 54 * Number(modelData.prompts || 0) / Math.max(1, ...Model.array((root.agent.usage || {}).days).map(d => Number(d.prompts || 0)))
                                            radius: 3
                                            color: root.swatch(index === 6 ? "teal" : "blue")
                                        }
                                    }
                                    Plain {
                                        width: parent.width
                                        text: String(modelData.date || "").slice(5)
                                        role: "caption"
                                        dim: true
                                        horizontalAlignment: Text.AlignHCenter
                                    }
                                }
                            }
                        }
                        Plain {
                            width: parent.width
                            text: "Prompt history · not subscription allowance history"
                            dim: true
                            role: "caption"
                        }
                    }
                    Card {
                        width: parent.width
                        visible: root.agent && Model.array((root.agent.usage || {}).models).length > 0
                        Plain {
                            text: "Model usage · available records"
                            strong: true
                        }
                        Repeater {
                            model: root.agent ? Model.array((root.agent.usage || {}).models) : []
                            Column {
                                required property var modelData
                                width: parent.width
                                spacing: 4
                                Plain {
                                    width: parent.width
                                    text: modelData.name
                                    strong: true
                                }
                                Pair {
                                    width: parent.width
                                    label: "Prompts today / week"
                                    value: Model.number(modelData.today_prompts) + " / " + Model.number(modelData.week_prompts)
                                }
                                Pair {
                                    width: parent.width
                                    label: "Input / output tokens"
                                    value: Model.number(modelData.input_tokens) + " / " + Model.number(modelData.output_tokens)
                                }
                                Pair {
                                    width: parent.width
                                    label: "Cached tokens"
                                    value: Model.number(modelData.cached_tokens)
                                }
                            }
                        }
                    }
                    Card {
                        width: parent.width
                        visible: root.agent && Model.array((root.agent.usage || {}).tools).length > 0
                        Plain {
                            text: "Frequent tools · available records"
                            strong: true
                        }
                        Repeater {
                            model: root.agent ? Model.array((root.agent.usage || {}).tools) : []
                            Pair {
                                required property var modelData
                                width: parent.width
                                label: modelData.name
                                value: Model.number(modelData.calls) + " calls"
                            }
                        }
                    }
                    Plain {
                        text: "Active & recent sessions"
                        dim: true
                        role: "caption"
                    }
                    Repeater {
                        model: root.agent ? root.agent.sessions : []
                        SessionCard {
                            required property var modelData
                            width: parent.width
                            session: modelData
                            agentName: root.name(modelData.agent)
                            identity: root.tone(modelData.agent)
                            onClicked: {
                                root.tab = "Sessions";
                                root.sessionId = modelData.id;
                            }
                        }
                    }
                    Card {
                        width: parent.width
                        Plain {
                            text: "Source & account"
                            strong: true
                        }
                        Pair {
                            width: parent.width
                            label: "Plan"
                            value: root.agent ? root.agent.quota.plan || "Not exposed" : ""
                        }
                        Pair {
                            width: parent.width
                            label: "Account"
                            value: "Not exposed"
                        }
                        Pair {
                            width: parent.width
                            label: "Usage scan"
                            value: root.agent ? Model.time((root.agent.usage || {}).updated * 1000) : ""
                        }
                        Pair {
                            width: parent.width
                            visible: !!root.agent && root.agent.id === "xai"
                            label: "Quota source"
                            value: root.agent ? root.agent.quota.source || "Not exposed" : ""
                        }
                        Pair {
                            width: parent.width
                            label: "Quota refresh"
                            value: root.agent ? Model.time(root.agent.quota.updated * 1000) : ""
                        }
                        Pair {
                            width: parent.width
                            label: "Source"
                            value: root.managerError ? "Disconnected · last known data" : "nyx-agentd · local records"
                        }
                        Pair {
                            width: parent.width
                            label: "Allowance history"
                            value: "Not exposed"
                        }
                    }
                }
                // SESSIONS
                Column {
                    width: parent.width
                    spacing: 9
                    visible: root.tab === "Sessions" && root.sessionId === ""
                    Controls.TextField {
                        id: sessionSearch
                        width: parent.width
                        placeholderText: "Search sessions or projects"
                        text: root.query
                        onTextEdited: root.query = text
                        Accessible.name: "Search sessions"
                        color: Nyx.Style.text
                        font.family: Nyx.Style.font
                        background: Rectangle {
                            color: Nyx.Style.field
                            radius: Nyx.Style.radiusSmall
                            border.color: Nyx.Style.border
                        }
                    }
                    SessionFilter {
                        width: parent.width
                        objectName: "session-agent-filter"
                        agents: root.agents
                        selectedAgentId: root.sessionAgentFilter
                        onAgentSelected: id => root.sessionAgentFilter = id
                    }
                    Plain {
                        text: "Live · " + root.matching().filter(Model.live).length
                        dim: true
                        role: "caption"
                    }
                    Repeater {
                        model: root.matching().filter(Model.live)
                        SessionCard {
                            required property var modelData
                            width: parent.width
                            session: modelData
                            agentName: root.name(modelData.agent)
                            identity: root.tone(modelData.agent)
                            onClicked: root.sessionId = modelData.id
                        }
                    }
                    Plain {
                        text: "Recent · " + root.matching().filter(s => !s.live).length
                        dim: true
                        role: "caption"
                    }
                    Repeater {
                        model: root.matching().filter(s => !s.live)
                        SessionCard {
                            required property var modelData
                            width: parent.width
                            session: modelData
                            agentName: root.name(modelData.agent)
                            identity: root.tone(modelData.agent)
                            onClicked: root.sessionId = modelData.id
                        }
                    }
                    Plain {
                        visible: root.matching().length === 0
                        text: "No matching sessions"
                        dim: true
                    }
                    Plain {
                        width: parent.width
                        text: "Live includes running and explicitly waiting sessions. Idle alone does not require attention."
                        dim: true
                        role: "caption"
                    }
                }
                Column {
                    width: parent.width
                    spacing: 10
                    visible: root.tab === "Sessions" && root.session !== null
                    Action {
                        text: "‹ Sessions"
                        onClicked: root.sessionId = ""
                    }
                    SessionCard {
                        width: parent.width
                        session: root.session || {}
                        agentName: root.session ? root.name(root.session.agent) : ""
                        identity: root.session ? root.tone(root.session.agent) : Nyx.Style.accent
                    }
                    Card {
                        width: parent.width
                        Pair {
                            width: parent.width
                            label: "Working directory"
                            value: root.session ? root.session.cwd || "Not exposed" : ""
                        }
                        Pair {
                            width: parent.width
                            label: "Branch"
                            value: root.session ? root.session.branch || "Not exposed" : ""
                        }
                        Pair {
                            width: parent.width
                            label: "Model"
                            value: root.session ? root.session.model || "Not exposed" : ""
                        }
                        Pair {
                            width: parent.width
                            label: "Last status"
                            value: root.session ? Model.time(root.session.updated) : ""
                        }
                        Pair {
                            width: parent.width
                            label: "Prompts"
                            value: root.session ? Model.number(root.session.prompts) : ""
                        }
                        Pair {
                            width: parent.width
                            label: "Tokens"
                            value: root.session ? Model.number(root.session.tokens) : ""
                        }
                    }
                    Action {
                        text: "View agent allowance & usage →"
                        onClicked: {
                            root.agentId = root.session.agent;
                            root.tab = "Agents";
                            root.sessionId = "";
                        }
                    }
                }
                // LOCAL AI: original hero, resource tiles and read-only model list.
                Column {
                    width: parent.width
                    spacing: 10
                    visible: root.tab === "Local AI"
                    Notice {
                        width: parent.width
                        visible: root.localError !== ""
                        text: "Nyx AI Core unavailable. " + (Object.keys(root.local).length ? "Last known status below." : "")
                        error: true
                    }
                    Card {
                        width: parent.width
                        Plain {
                            text: "Nyx AI Core"
                            strong: true
                        }
                        Pair {
                            width: parent.width
                            label: "Worker"
                            value: (root.local.worker_status || {}).state || "Unavailable"
                        }
                        Pair {
                            width: parent.width
                            label: "Backend"
                            value: Model.array((root.local.resource_status || {}).gpu_backends).join(", ") || "Not exposed"
                        }
                        Pair {
                            width: parent.width
                            label: "Uptime"
                            value: root.local.worker_status ? Math.floor(root.local.worker_status.uptime_secs / 60) + " min" : "Unavailable"
                        }
                        Pair {
                            width: parent.width
                            label: "Endpoint"
                            value: (root.local.service_settings || {}).openai_endpoint || "Not exposed"
                        }
                    }
                    Grid {
                        width: parent.width
                        columns: 2
                        spacing: 8
                        Repeater {
                            model: [
                                {
                                    k: "System RAM available",
                                    v: Model.gib((root.local.resource_status || {}).ram_available),
                                    s: "of " + Model.gib((root.local.resource_status || {}).ram_total) + " system RAM"
                                },
                                {
                                    k: "GPU memory capacity",
                                    v: Model.gib((root.local.resource_status || {}).vram_total),
                                    s: "Used VRAM not exposed"
                                },
                                {
                                    k: "Queue",
                                    v: Model.number((root.local.resource_status || {}).queued),
                                    s: "requests waiting"
                                },
                                {
                                    k: "Worker requests",
                                    v: Model.number(((root.local.worker_status || {}).metrics || {}).requests),
                                    s: "since worker started"
                                }
                            ]
                            Card {
                                required property var modelData
                                width: (parent.width - 8) / 2
                                Plain {
                                    width: parent.width
                                    text: modelData.k
                                    dim: true
                                    role: "caption"
                                }
                                Plain {
                                    width: parent.width
                                    text: modelData.v
                                    size: 22
                                }
                                Plain {
                                    width: parent.width
                                    text: modelData.s
                                    dim: true
                                    role: "caption"
                                }
                            }
                        }
                    }
                    Plain {
                        text: "Loaded models"
                        dim: true
                        role: "caption"
                    }
                    Repeater {
                        model: Model.array((root.local.resource_status || {}).loaded)
                        Card {
                            required property var modelData
                            width: parent.width
                            Plain {
                                width: parent.width
                                text: modelData.id
                                strong: true
                            }
                            Pair {
                                width: parent.width
                                label: "State"
                                value: modelData.state
                            }
                            Pair {
                                width: parent.width
                                label: "Active / slots"
                                value: modelData.active + " / " + modelData.slots
                            }
                            Pair {
                                width: parent.width
                                label: "Estimated model memory"
                                value: Model.gib(modelData.estimate)
                            }
                        }
                    }
                    Plain {
                        text: "Other available models"
                        role: "caption"
                        dim: true
                        visible: root.availableModels().length > 0
                    }
                    Repeater {
                        model: root.availableModels()
                        Card {
                            required property var modelData
                            width: parent.width
                            Plain {
                                width: parent.width
                                text: modelData.id || modelData.name || "Model"
                                strong: true
                            }
                            Pair {
                                width: parent.width
                                label: "State"
                                value: modelData.state || "Installed"
                            }
                            Plain {
                                width: parent.width
                                text: Model.array(modelData.capabilities).join(" · ")
                                dim: true
                                role: "caption"
                            }
                        }
                    }
                    Plain {
                        visible: Model.array((root.local.resource_status || {}).loaded).length === 0
                        text: "No loaded model data"
                        dim: true
                        role: "caption"
                    }
                    Card {
                        width: parent.width
                        Plain {
                            text: "Performance · worker averages"
                            strong: true
                        }
                        Pair {
                            width: parent.width
                            label: "Tokens per second"
                            value: ((root.local.worker_status || {}).metrics || {}).tokens_per_second_avg !== undefined ? Number(root.local.worker_status.metrics.tokens_per_second_avg).toFixed(1) : "Unavailable"
                        }
                        Pair {
                            width: parent.width
                            label: "First-token latency"
                            value: ((root.local.worker_status || {}).metrics || {}).first_token_ms_avg !== undefined ? root.local.worker_status.metrics.first_token_ms_avg + " ms" : "Unavailable"
                        }
                        Pair {
                            width: parent.width
                            label: "CPU / GPU utilization"
                            value: "Not exposed"
                        }
                        Pair {
                            width: parent.width
                            label: "Context utilization"
                            value: "Not exposed"
                        }
                        Pair {
                            width: parent.width
                            label: "Last worker error"
                            value: (root.local.worker_status || {}).last_error || "None reported"
                        }
                    }
                    Plain {
                        width: parent.width
                        text: "System memory availability and model estimates are not Core process memory consumption."
                        dim: true
                        role: "caption"
                    }
                }
            }
        }
        Plain {
            Layout.fillWidth: true
            text: root.refreshed ? "Last refresh attempt · " + Model.time(root.refreshed) : "Waiting for telemetry"
            dim: true
            role: "caption"
        }
    }
    Shortcut {
        sequence: "Ctrl+F"
        enabled: root.active
        onActivated: {
            root.go("Sessions");
            sessionSearch.forceActiveFocus();
        }
    }
    Keys.onEscapePressed: {
        if (tab === "Settings")
            go("Agents");
        else if (agentId !== "")
            agentId = "";
        else if (sessionId !== "")
            sessionId = "";
        else
            closeRequested();
    }
}
