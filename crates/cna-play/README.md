# cna-play

A bounded server integration launcher: one Claude Code Haiku seat (Axis commander), nine
aggressive scripted seats, and the production spectator board. This is a synthetic
sandbox game, not a playable CNA scenario or the production multi-seat scheduler.

Build the board once with `npm --prefix web ci` and `npm --prefix web run build`.
Select the owner's **claude-5** profile through `CNA_CLAUDE_CONFIG_DIR` and set
`CNA_CLAUDE_EMAIL` to the expected login email. No credential file is read or copied
by the launcher. The driver checks the expected login before starting model work.

```powershell
$env:CNA_LIVE_CLI_TESTS = '1'
# Set CNA_CLAUDE_CONFIG_DIR and CNA_CLAUDE_EMAIL for claude-5.
cargo run -p cna-play
```

Open the printed board URL before the ten-second startup delay ends. The CLI uses
only its token-scoped MCP endpoint; its empty working folder and MCP configuration
are outside the repository. The campaign database and transcript JSONL remain in the
printed temporary directory. The launcher pauses after at most two model turns,
keeps the HTTP viewer open for twenty seconds, and joins the campaign writer before exit.

Limits are explicit: one paid session, Haiku, two turns, forty MCP calls, sixty
seconds per turn and 150 seconds total model work. A failed, timed-out or unanswered
turn records a durable seat failure and leaves its pending decision unanswered.
There are no retries or substituted orders in this probe. Binding changes stop
the old session; the endpoint's fixed controller epoch rejects stale submissions.
The campaign pauses at the end of this bounded run, including after a handover.

The transcript sink writes directly to `CampaignHandle`. SQLite assigns each seat's
`tseq` and perspective-specific game alignment; the existing HTTP/WebSocket service
sends those entries to the board's transcript panel. Transcript fixtures contain
protocol entries rather than raw CLI stdout.

`cargo test -p cna-play` uses a fake CLI that calls the real HTTP MCP server and checks
the persistent live WebSocket transcript. It also checks timeout, terminal completion, handover, authentication cancellation and unavailable-writer cleanup.
`live_haiku_server_probe_is_opt_in` returns before creating a campaign or touching
a CLI unless `CNA_LIVE_CLI_TESTS=1`; its live mode uses one turn with the same account
requirements. CI never sets that flag.

Known limits: the launcher currently fixes the seat/model choice, stores no durable
CLI session id for launcher restart, and stops after two turns. General per-seat
launch arguments, crash recovery and campaign scheduling are later milestones.
Saved runs can be inspected without a CLI or a paid opt-in:

```sh
cargo run -p cna-play -- --replay /path/to/campaign.sqlite
```

The replay viewer serves the recovered campaign for thirty seconds. The measured
Haiku fixture in `tests/fixtures/claude_sandbox_transcript.jsonl` contains 22 entries,
three accepted commander orders and eight paired tool calls. Its account email and
session identifier are redacted. The six-turn sandbox finished within one CLI turn;
reported cost was about $0.038. The production board displayed all transcript entries
without browser errors. A regression checks that finishing in one turn succeeds.

Shutdown waits at most five seconds for transcript delivery. If the writer remains
unavailable, it stops delivery and saves unconfirmed captures in
`<campaign-id>.axis.commander.unconfirmed.jsonl` beside the campaign. These captures have no assigned
`tseq`; the last append may already have committed without confirmation. Inspect the
campaign before replaying them. A failed outbox write is reported explicitly. The
HTTP and MCP services still close when persistence fails.
A CLI failure and a recovery failure are both reported when they occur together.

The printed board URL includes an operator capability in its `#cap` fragment.
Keep that URL with trusted spectators. The capability stays in the launcher and
board; no credential file is written, and the driver receives only its independent
seat-scoped MCP URL. Inherited `CNA_*` variables, including capability paths/tokens,
are scrubbed from child environments. Each invocation creates a fresh campaign
directory. Engine fingerprints reject incompatible older saved databases; the
replay command does not migrate them.