# cna-play

A campaign launcher with a production spectator board and per-seat controllers.
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
Human/scripted-only runs have a separate unpaid scheduling limit; paid limits stay unchanged.

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

Shutdown waits at most five seconds for transcript delivery. A failed writer or a
backlog from a fast seat that exceeds this deadline causes
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

For a campaign session, use explicit lifetime budgets:

```sh
cargo run -p cna-play -- --kind cna --seat axis.front_line=claude:haiku --seat '*=scripted:pass_when_possible' --long-lived --turns 2048 --tool-calls 16000 --wall-seconds 21600 --turn-timeout 120 --context-tokens 100000 --recoveries 3
cargo run -p cna-play -- --resume /path/to/campaign.sqlite
```

`--long-lived` keeps one CLI session across decision windows. Its defaults remain
small: two model turns, forty calls, 150 seconds total and sixty per turn. Explicit
bounds may extend to 8192 turns, 100000 calls, six hours, twenty minutes per turn,
and eight recoveries, with at most two concurrent Claude seats. Resume accepts only
the database path and reuses the original limits, controller epochs and models;
it cannot replace limits or reset counters. A changed or failed binding refuses
resume. Ctrl-C stops the current run, pauses the campaign and preserves the journal.
Budget exhaustion or CLI failure pauses the original binding without fallback orders.

The trusted SQLite journal lives in `session-journals/<campaign-id>.sqlite` under
the campaign directory. An exclusive SQLite lease prevents duplicate launchers and
releases on process exit, including a hard kill. The journal commits a turn/time
reservation before each operation and charges each MCP call before dispatch. A hard
crash retains the whole unfinished time reservation; offline downtime is excluded.
Idle time, authentication and model turns count toward the lifetime wall limit.
Unconfirmed result-less turns are marked as incomplete telemetry. Preserve the
campaign, journal and original per-seat sandbox paths together for resume. Campaign
fingerprints still reject incompatible engine/content versions; no migration is implied.

Each turn authorizes exactly one decision ID and revision. If a group reopens with
another revision, the next model turn handles it. A process death resumes the same
CLI session; an explicitly unavailable resume starts a new session from the game's
durable notebook, preserving all budgets. Authentication, quota, isolation and turn
errors pause instead of triggering a notebook fallback. The first prompt after a
restart includes the durable notebook, and notebook failures stop model work.

Claude owns automatic compaction. The launcher sets the effective context window
(`--context-tokens`, 16000..200000) and an 80% native compaction trigger. It captures
`compact_boundary` events and current API-request context occupancy separately from
aggregate turn usage. If occupancy still reaches 95% of the configured window at a
turn boundary, the session pauses. An idle Claude process gets at most two seconds
of stdin EOF grace to persist resumable totals; a canceled active turn is killed.
See [Claude environment variables](https://code.claude.com/docs/en/env-vars).

Journal stage entries count requested decision attempts/completions and reported
input/output tokens. Cost uses differences of Claude's cumulative session estimates,
including restored resume totals; it does not sum successive cumulative values.
A reset/reseed preserves earlier spend and uses a new baseline. Stage totals describe
only this seat's requested windows, including partial stages. Missing results can
omit spend, so these are estimates and incomplete telemetry is explicit; the CLI
report is not an authoritative bill. See [Claude cost tracking](https://code.claude.com/docs/en/agent-sdk/cost-tracking).

Bounded default tests complete a few requests and verify recovery. Whole-CNA runs
are marked slow and run in the CI slow-test job. Offline tests cover original-session resume, notebook reseed, repeated real CNA
windows, hard-crash reservations, persisted tool caps, lease exclusion, corrupt
journals and stale bindings. Live checks remain opt-in and bounded. Codex and
Antigravity still await confinement proofs and drivers.

The measured durable proof used one Claude Code 2.1.289 / Haiku 4.5 session,
restarted the launcher, and resumed the same CLI session. It accepted two nonempty
Maletti unit-movement orders in GT1:OpStage1, with 15 paired MCP calls and 54
transcript entries. The notebook survived restart. One missing revision was
rejected, then corrected before acceptance. Only the nine scoped MCP tools were
exposed. Recorded context occupancy was 23357 tokens; no native compaction was
needed in this short check.

The reported cumulative estimate was $0.1009509: $0.0550391 for the first response
and $0.0459118 after resume, about $0.05048 per answered movement window. This is
a partial OpStage sample, with nine scripted seats passing and units subject to
ration/water restrictions. It does not measure a full supplied OpStage or a
subscription invoice. Accounting records three reserved attempts, two completed
model turns and one canceled inert attempt; that third reservation made no paid
CLI call. Sanitized transcript and accounting fixtures preserve this evidence.
