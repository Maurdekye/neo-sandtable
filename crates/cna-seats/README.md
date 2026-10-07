# cna-seats

Seat-scoped MCP tools, CLI drivers, transcript capture and a budgeted seat runner.
The asynchronous `GameBackend`, `SeatMemory` and `TranscriptStore` interfaces are
implemented both by the synthetic Number Duel test backend and by cna-server's
`CampaignHandle`. No provider API or orgtree runtime is used.

## CLI status

| CLI | Verified support | Remaining work |
| --- | --- | --- |
| Claude Code 2.1.289 | One headless stream-json process across turns, resume by session id, account verification, MCP-only tools; two Haiku seats completed Number Duel on claude-5 | Additional CLI adapters; full CNA rules remain in development |
| Codex 0.160.0 | Local spike verified exec/resume thread continuity and a restricted feature configuration | Driver draft is preserved locally; not yet shipped |
| Antigravity | Prior orgtree reference research suggests streamed stdin and conversation-id resume | Headless and isolation probes; driver |

## Claude Code

The driver launches `claude -p --input-format stream-json --output-format stream-json
--verbose`, holds stdin open, sends one user JSON line per turn and stops reading a turn
at its `result` event. `--resume` restores an existing session. The durable launcher seeds a new
session from the game notebook when the CLI explicitly reports an unavailable resume.

Isolation flags: `--tools "" --strict-mcp-config --allowedTools "mcp__cna__*"
--permission-mode dontAsk --setting-sources "" --disable-slash-commands`.
A replacement system prompt explains the seat and game tools. The driver's init
check refuses tools outside its own MCP server and an explicitly disconnected server.
Do not use `--safe-mode`: the local probe found it restores built-in tools.
The CLI runs in an empty per-seat temporary directory outside the repository.

Use the owner's claude-5 account: set `ClaudeConfig.config_dir` to that profile
(`CLAUDE_CONFIG_DIR` in the spawned process) and `expected_email` to its expected login.
Environment scrubbing removes parent CLI identifiers and provider credentials before
the selected account directory is added. The driver never copies credentials.

## Transcripts and budgets

Assistant text, exposed reasoning, tool call/result pairs, accepted decisions and system
entries pass to `TranscriptSink`. The store owns per-seat numbering and persistence.
Haiku's measured reasoning blocks exposed signatures without thinking text; no reasoning
text is invented. Quota and usage values are parsed separately from game adjudication.

`SeatRunner` caps concurrent sessions, tool calls, run wall time and turn time; missing
answers and exhausted limits pause decisions. Parking sessions between windows retains
the CLI session id while releasing the concurrency slot. The server-backed bounded
launcher is in [cna-play](../cna-play/README.md), supporting both sandbox and Graziani development campaigns with per-seat bindings. Its opt-in durable mode keeps session ids and lifetime budgets across campaign/process restarts.

## Verification

`cargo test -p cna-seats` runs parser fixtures, tool isolation, hidden-information,
idempotency, recovery and runner tests without a real CLI. See
`tests/fixtures/claude_two_turns.jsonl` for the sanitized raw Claude stream.
The cna-play integration test additionally checks real MCP HTTP calls and live persisted
WebSocket transcript messages. Paid probes require `CNA_LIVE_CLI_TESTS=1` and use Haiku.
The authentication subprocess is killed when its start future is cancelled, including
handover before model startup. Transcript delivery retries temporary store failures
in capture order. The generic runner bounds its final drain at five seconds and reports retained unconfirmed captures on failure. Supervisors can stop delivery, join the worker and retrieve the
unconfirmed captures for an outbox; an in-flight append may have committed before
cancellation, so recovery must check stored entries before replaying captures.
Operator HTTP capabilities belong only to the trusted launcher and spectator board.
Drivers receive their independent seat-scoped MCP URL. All inherited `CNA_*`
variables are removed along with parent agent and provider credentials, including
operator capability paths and source-directory paths. No operator credentials are
written to the seat working directory or included in its prompt/MCP configuration.