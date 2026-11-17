# AI Lens

Independent, read-only Quickshell panel. AI Hub is unchanged. `nyx-ai-lens` opens/toggles the panel; `--stdio` is the private QML companion transport and `--snapshot` reads a single telemetry snapshot.

The UI implements the selected A direction in `nyx-design/mockups/ai-lens-selected`: the original four-column summary, stacked agent cards and independently named remaining quota pools, useful agent detail, Live/Recent session groups with inline agent identities, and the original Local AI resource/model layout without model management. All runtime values come from services. Missing metrics stay unavailable; stale quota windows retain their source error and refresh timestamp. Prompt/token usage is distinct from subscription allowance. Idle alone is never an attention request.

Agent data uses nyx-agentd's versioned public control protocol. AI Core is optional and uses its existing resource/worker/model status interfaces. Background polling runs only while visible; provider quota access remains subject to nyx-agentd's existing opt-in setting. The bridge never reads credentials, launches agents, stops sessions, or loads/unloads models.

`../../install-ai-lens.sh` from the repository root builds and installs the binary, app-menu entry/icon, Omarchy adapter, and discoverable Nyx Shell component without opening it. The Omarchy plugin ID is `design-nexus.ai-lens`. Both launch through the same QML view; no NyxOS-only service is mandatory. A native Quickshell IPC handler also exposes `nyx-ai-lens` show/hide/toggle for shell bindings.

Design 2.0 Ui/Theme modules are vendored from the revision in `qml/nyx/vendor.json`, with relative-import adaptation for Omarchy. They read the live system-theme snapshot. No plugin-specific theme setting is introduced.

Validation: release build, live read-only manager/AI Core snapshot, native offscreen loader/render with illustrative data. Omarchy/NyxOS popup placement and physical keyboard interaction require a live user-session check; installation deliberately does not open or restart an app.

The header gear opens persistent agent switches (`~/.config/nyx-ai-lens/settings.ini`, honoring XDG_CONFIG_HOME). Choices filter cards, sessions and overview counts; they do not change global provider opt-ins in nyx-agentd. Installed harnesses without usage records or quota windows start hidden, and can be explicitly enabled. Overview quota rows match the original compact density; reset countdowns are available on hover and in detail. Refresh/session timestamps use relative ages instead of calendar dates.

Grok Build and Cursor CLI are presented together as xAI, with one shared weekly allowance. The provider-wide Grok weekly reading is preferred; a successful Cursor weekly reading can replace a failed source. Product-share, monthly, API and on-demand windows are not added or relabeled. Usage records and sessions retain their source client, and the old display switches migrate into one xAI switch. Shared-plan reference: https://docs.x.ai/x-subscription/usage-limits
