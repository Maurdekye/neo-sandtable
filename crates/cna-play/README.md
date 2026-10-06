# cna-play

A bounded campaign launcher with a production spectator board and per-seat controllers.
`--kind sandbox` selects the synthetic integration game. `--kind cna` selects Graziani's
Offensive under `cna-2021-dev`: implemented procedures produce real decisions; the
profile skips unfinished procedures and is not a complete CNA rules simulation.

```powershell
# Scripted CNA; no CLI or paid opt-in needed.
cargo run -p cna-play -- --kind cna --seat '*=scripted:legal_random'

# One Claude seat on the owner's claude-5 account.
$env:CNA_LIVE_CLI_TESTS = '1'
# Set CNA_CLAUDE_CONFIG_DIR to claude-5 and CNA_CLAUDE_EMAIL to its expected login.
cargo run -p cna-play -- --kind cna --seat axis.commander=claude:haiku --seat '*=scripted:legal_random' --turns 1 --tool-calls 8
```

Build the board with `npm --prefix web ci` and `npm --prefix web run build`.
Open the printed URL during the ten-second startup delay. Its `#cap` fragment is
operator authority: keep it with trusted spectators. It stays in the launcher and
board; no credential file is written. Drivers receive only independent, seat-scoped
MCP URLs, with built-in file, shell and network tools disabled and empty per-seat
working directories outside the repository. Inherited `CNA_*` capability paths,
source paths and parent provider/agent credentials are scrubbed. The driver checks
the expected Claude login before model work; it never copies credentials.

Bindings accept `claude:MODEL`, `scripted:legal_random`,
`scripted:pass_when_possible`, `human`, and (sandbox only) `scripted:aggressive`.
A `*` binding supplies every unspecified seat. Explicit seats override it regardless
of argument order; duplicate wildcard/seat bindings are rejected. With no `--seat`
flags, Axis commander is Haiku and the other seats use the kind's scripted default.
With any binding flags, unspecified seats use the scripted default. All ten canonical
seat IDs use `axis` or `commonwealth`; roles are `commander`, `logistics`, `rear_area`,
`air`, and `front_line`. Codex and Antigravity are rejected until confinement is proven.

The launcher permits at most two Claude seats, each with one CLI process, up to two
model turns, forty tool calls, sixty seconds per turn and 150 seconds of model work.
`--turns 1|2` and `--tool-calls 1..40` can lower those bounds. Paid probes use one
Haiku seat. When any session completes its bounded allocation or fails, peers stop
and the campaign pauses; unfinished decisions stay unanswered. A timeout, missing
answer or CLI refusal records failure for its bound epoch. No fallback orders are
submitted. A handover cancels the old session without marking its replacement failed.
Human/scripted-only runs wait for completion up to the same campaign wall limit.

Prompts consume the actual pending decision IDs and the seat notebook, and direct the
CLI to `observe`, `describe_actions`, `validate`, and `submit`. They do not assume a
specific decision kind or encode an initiative answer. Each model turn requests only
its listed window IDs; later windows belong to later turns. The notebook, team tools
and engine-filtered observation work the same way for sandbox and CNA.

SQLite assigns transcript numbering/alignment. Live HTTP/WebSocket streams feed the
board panel. Every CLI seat's protocol transcript is exported to a named JSONL file
in the fresh, persistent temporary directory printed at startup. The viewer stays
open for twenty seconds after the run. Save the campaign database for inspection:

```sh
cargo run -p cna-play -- --replay /path/to/campaign.sqlite
```

Replay dispatches either campaign kind and serves it for thirty seconds without a
CLI or paid opt-in. Complete engine/content fingerprints reject incompatible saved
databases; this command does not migrate them.

Shutdown waits at most five seconds for transcript delivery. A failed writer causes
delivery to stop and unconfirmed captures to be saved to
`<campaign-id>.unconfirmed.jsonl`. These have no assigned transcript sequence; an
in-flight append may have committed without confirmation. Inspect stored entries
before replaying captures. Primary CLI, control, persistence and outbox-write errors
are all preserved. HTTP/MCP services close even when persistence fails.

Offline tests cover parser rejection, real CNA observations/action schemas, scripted
completion/recovery, authenticated live transcripts, binding handover, cancellation,
bounded sibling stop, and writer failure/outbox recovery. Live tests return before
creating a campaign or touching a CLI unless `CNA_LIVE_CLI_TESTS=1`; CI never sets it.
The measured sandbox fixture contains 22 entries, three accepted commander orders
and eight paired tools, with account/session redacted; reported cost was ~$0.038.

A measured CNA run on Claude Code 2.1.289 / Haiku 4.5 accepted one initiative
order through five paired tools, including a standing-plan notebook write. Its
18-entry protocol fixture is saved with account/session redacted; reported cost
was \$0.022854. The production board displayed all 18 entries without page errors.
The campaign paused at the next OpStage decision, leaving it unanswered.

Known limits: this remains a bounded launcher, with no durable launcher session ID
or restart/resume scheduler. Campaign-long sessions, compaction/context budgets and
notebook-based recovery are the next milestone, followed by Codex and Antigravity.