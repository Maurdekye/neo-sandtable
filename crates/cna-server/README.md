# cna-server

Run the local server from the repository:

```sh
cargo run -p cna-server
```

It listens on `127.0.0.1:3000`, recovers existing campaign databases and serves `web/dist` when
present. Set `CNA_PORT`, `CNA_DATA_DIR` and `CNA_CAMPAIGN_DIR` to override the port, published data
folder and database folder. The default database folder is the agent scratch folder's
`campaigns/`, outside the repository. Browser requests require a loopback Host and an approved
local Origin; the Vite origins `http://localhost:5173` and `http://127.0.0.1:5173` are allowed.

Every API request requires a capability. Startup generates fresh high-entropy credentials,
prints `http://127.0.0.1:PORT/#cap=TOKEN`, and writes a trusted launcher credential document to
`CNA_CAMPAIGN_DIR/operator-capabilities.json` (override with `CNA_CAPABILITY_FILE`). The fragment
is for the board to capture; it is not sent in HTTP request lines. The file refreshes after campaign
registration. Credentials rotate on every `App::new` / server restart; saved campaigns retain their
state but clients need newly issued capabilities. The static board assets contain no credentials.

Use `Authorization: Bearer TOKEN` for HTTP. Browser WebSockets use
`/api/campaigns/{id}/stream?cap=TOKEN`; native clients may use the Bearer header. Never log that
WebSocket URI or expose it in error text. Conflicting header/query credentials are rejected.
Missing, malformed or unknown credentials return 401. Valid credentials outside their campaign
or perspective scope return 403. An unauthorized WebSocket subscription or perspective switch
closes with code 1008 and reason `capability scope denied` before sending any unauthorized data.
Host/Origin checks remain active. Approved CORS preflights need no token; application requests do.
API responses use `Cache-Control: no-store`; responses use `Referrer-Policy: no-referrer`.

`GET /api/session` returns `{perspective, campaign_id, operator}`. Only operator authority can
create/control campaigns, hand over or pause seats, or fetch
`GET /api/campaigns/{id}/capabilities`, which returns `{sides, seats}` credentials for that campaign.
Side capabilities read only their side and its seats. Seat capabilities read only that exact seat
and may submit its decisions; side credentials cannot submit. Every restricted credential is bound
to one campaign. Metadata endpoints still require an explicit authorized `perspective` when using
a restricted token; their default is operator. HTTP transcript requests also require an authorized
seat. Client discovery is filtered to the capability's campaign.

Trusted Rust launchers keep `App::operator_token()` in memory and may print the fragment URL without
creating a credential file. `App::side_token(id, side)` and `App::seat_token(id, seat)` return
`Option<String>` after registration. Optional `App::write_credentials(path)` is synchronous and
returns `Result<(), cna_server::Error>`; it atomically exports the operator and all campaign scopes.
Never pass the operator capability, export path, environment or working directory to a seat CLI.
Capabilities require seat confinement: each driver has an empty seat directory, built-in shell,
file and network tools disabled, and only its own token-scoped MCP endpoint. A driver that cannot
provide this confinement must not take a seat. Unrestricted same-user processes can read files or
process memory; bearer tokens alone cannot protect against that authority. The direct bound-epoch
`CampaignHandle` MCP bridge does not require an App capability.

Creation accepts JSON with `kind` (`sandbox`, the default, or `cna`), `rules_profile`, `seed`
(32 byte values), optional `title` (default "Campaign"), `paused` (default false), and `controller`.
Use `paused: true` to attach viewers before starting a fast campaign. No LLM driver or paid
provider is started by the server.

- `kind: "sandbox"`, `rules_profile: "sandbox-v1"` runs the **synthetic game**. Controllers:
  `legal_random`, `pass_when_possible`, `aggressive` / `scripted:aggressive`, or `human`.
- `kind: "cna"`, `rules_profile: "cna-2021-dev"` runs the real Graziani's Offensive setup and
  sequence through the development ruleset. It resolves supported initiative and movement windows and
  skips unimplemented steps; reaching Finished does not mean all CNA rules are implemented.
  Controllers: `legal_random`, `pass_when_possible`, or `human`.
- `kind: "cna"`, `rules_profile: "cna-2021-full"` uses the strict ruleset and stops visibly at the
  first unsupported applicable procedure. It never substitutes an order. Aggressive is available
  only for the sandbox (including handover). Unknown kinds, profiles and mismatched pairs are rejected.

CNA legal-random movement uses `cna_rules::baseline::random_orders` inside the campaign writer,
with a deterministic controller-local RNG seeded by request and controller epoch. It chooses
a declared pass with the usual one-in-five legal-random probability, otherwise one legal
complete unit path or an empty order list. Creation and recovery install the same
policy; pass-when-possible remains a separate controller, and aggressive stays sandbox-only.
Rejected policy actions pause the seat after one attempt, without a substitute or retry.

Legal-random samples enumerable schemas and preserves ruleset-provided hex/path candidate hooks.
Without such a hook, a schema containing unenumerated hexes or paths uses its declared pass as
an explicit baseline policy. If no pass exists, the seat pauses immediately with the decision
kind and missing-domain reason. A rejected pass also pauses without retries; generation or
execution failures never trigger a substitute order. Controller sampling does not consume
campaign adjudication RNG.

For example, POST `/api/campaigns` with `{"kind":"cna","rules_profile":"cna-2021-dev",
"seed":[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],"paused":true,"controller":"legal_random"}`. Rust callers supply
`CreateRequest.kind: CampaignKind::Sandbox` or `CampaignKind::Cna`; legacy JSON without `kind`
still creates a sandbox. `campaigns::create` dispatches both kinds; `sandbox::create` remains the
sandbox-specific factory.

| HTTP endpoint | Purpose |
|---|---|
| `GET /api/session` | Current capability scope |
| `GET /api/campaigns/{id}/capabilities` | Operator-only restricted credential issuance |
| `GET/POST /api/campaigns` | List metadata / create a campaign |
| `GET /api/campaigns/{id}` | Inspect metadata, status and projected snapshot |
| `POST /api/campaigns/{id}/pause` or `/resume` | Campaign control at a safe boundary |
| `GET /api/campaigns/{id}/seats` | Seat metadata |
| `POST /api/campaigns/{id}/seats/{seat}/controller` | Handover `{controller, config}`; returns new epoch |
| `POST /api/campaigns/{id}/seats/{seat}/pause` | Pause one seat without substituting an order |
| `GET /api/campaigns/{id}/seats/{seat}/observe` | Authorized observation, pending requests, epoch and failure |
| `GET /api/campaigns/{id}/seats/{seat}/inspect/{target}` | Authorized ruleset detail |
| `GET /api/campaigns/{id}/seats/{seat}/decisions/{decision}/actions` | Request and action schema |
| `POST /api/campaigns/{id}/seats/{seat}/decisions/{decision}/validate` | Pure draft validation `{action}` |
| `POST /api/campaigns/{id}/seats/{seat}/decisions/{decision}/submit` | Full core `DecisionResponse` |
| `GET /api/campaigns/{id}/transcripts?perspective=...&seat=...&after=...` | At most 512 authorized transcript rows |
| `GET /api/campaigns/{id}/stream` | WebSocket implementing `docs/protocol.md` |

Metadata queries accept `?perspective=operator|side:...|seat:...`; the default is the local
operator. The WebSocket accepts `subscribe`, then emits `hello`, a fresh `snapshot` or events
after `from_seq`, authorized historical transcripts, and live events/transcripts. A gap or a
lagged subscriber gets `resync`; resubscribe with `from_seq: null` for a fresh snapshot. Slow
writes time out and close the connection. Transcript reconnect replay is identified by
`(seat,tseq)`; clients should deduplicate those entries. Each historical catch-up has a fixed
upper cursor even when new entries keep arriving.

`CampaignHandle` is a bounded actor interface and implements the asynchronous `cna-seats`
`GameBackend`, `SeatMemory` and `TranscriptStore`. `watch_seat(seat)` publishes only changes to
that seat's authorized pending requests, observation or binding, including reissues after
handover. Controller callbacks use `mark_failure_if_epoch(seat, expected_epoch, reason)` to
durably pause a failed controller: the writer rejects a superseded epoch without pausing its
replacement or adding a transcript. `mark_failure(seat, reason)` and `pause_seat` remain
unconditional operator controls.
`handover` installs a controller and a new epoch and clears its pause. `shutdown` joins the
writer thread. Submit receipts are stable acknowledgements; observations are fetched separately.
Scripted baselines emit factual `decision_submitted` entries with the accepted action, and
`system` entries when they pause; they do not generate model commentary. A scripted submission's
entry commits atomically with its answer and is not duplicated on an idempotent retry.
The legacy unscoped `GameBackend::game_seq()` returns zero; use a seat projection's sequence
instead. Transcript alignment is assigned inside the persistent store.

One OS thread exclusively owns each `Campaign<R>`, its game and its SQLite connection. Accepted
commands, audience-tagged events, all 13 contiguous perspective streams, pending windows, RNG,
revision, lifecycle and optional checkpoint commit in one transaction. The in-memory game
changes only after commit. Rejections consume no campaign randomness. Exact idempotent replays
are acknowledged; reuse for a different command is refused. Handover preserves accepted secret
answers. Scripted controllers use their own deterministic randomness and pause after bounded
retries; `spawn_with_candidates` accepts the ruleset adapter's hook for paths and arbitrary hexes.

Checkpoints are made every 32 accepted commands. Recovery verifies immutable input pins, the
checkpoint and every later re-evaluated state/RNG/transition hash. Seat notes, team messages,
controller epochs, failure state and transcript counters survive recovery. Team-message numbers
are per recipient; game events and transcript alignment are per perspective, so neither exposes
hidden activity through sequence gaps.

Live game/transcript channels have 128 slots and never wait for a viewer. Snapshots are immutable
committed projections; replay pages use independent read-only SQLite connections off the writer.
The tests exercise rollback injection, recovery, stale epochs, exact duplicates, secret state,
13-perspective filtering, a real-map sandbox reaching Finished with an unread lagged viewer,
durable seat memory and HTTP/WebSocket snapshot/live/resume/switch/resync flows.

CNA startup recovery dispatches from the persisted scenario/profile pair using a read-only
metadata query before opening a writer. Its input fingerprint comes from
`cna_rules::content::source_files(data_dir, "graziani")`: the loaders record every file they
actually read, including conditional movement-layer/provenance inputs, areas, and inherited
scenario setup files. Notes and unread scenarios do not affect it. The sorted, deduplicated
manifest replaces the server's manual file list. Hashing uses normalized relative paths and
bytes before and after the real content load; a change during loading is rejected.
Both CNA and sandbox use the same complete engine fingerprint, embedded at
build time from core, protocol, content, tables, rules and sandbox Rust sources, their manifests,
the workspace manifest and Cargo.lock. It includes helpers such as hex geometry and dependency
versions. Directory watches catch newly added source files. Changed pins are rejected before
interpreting a checkpoint, including campaigns with no later commands. There is no migration or
implicit profile fallback: databases saved with the older partial sandbox fingerprint (or earlier
CNA fingerprint) are intentionally rejected. Use a fresh campaign directory for new campaigns;
do not relabel saved pins to bypass that check.
The real-scenario tests cover both baselines reaching Finished, private transcript replay,
checkpoint plus tail recovery, strict-profile rollback, HTTP creation/control, input drift and
mixed sandbox/CNA recovery. No provider invocation is needed for these tests.

Default CNA checks use bounded decision windows, including an accepted real-unit move and
mid-run recovery of all perspective views, counters, transcripts and adjudication RNG. Whole
Graziani campaigns are marked `slow: whole campaign`; run them with
`cargo test -p cna-server -- --ignored`. Completed server CI on commitf16f459
(run37564332764) measured legal-random397.550 seconds for3407commands,
pass-when-possible191.842 seconds for1735commands, and HTTP legal-random423.796 seconds.
Ignored-only ceilings are800,400 and850 seconds respectively, about twice each completed
measurement. Those runs predate force-assignment and mandatory breakdown windows; recalibrate
from their next completed CI report. Default and paid-driver limits stay unchanged.
The bounded mover test prints engine, writer (including engine and SQLite), and all-perspective
projection timings; it uses real unit data with a small test map rather than a full-roster benchmark.

One bounded fixture run resolved 16 decisions in 31 transitions: engine evaluation averaged
1.1 ms, the writer including engine and SQLite averaged 29.2 ms, and all 13 projections averaged
1.7 ms per resolved decision. These include automatic transitions between decisions. The test
uses real Graziani unit data and a small map; this is not a full-roster benchmark, and SQLite
cost was not measured independently from the writer. Numbers vary with machine load.

Ignored whole-campaign tests emit `CNA_PROFILE` on CI with wall time, accepted command count,
resolved decision counts by kind, and runtime engine, durable writer, projection and controller
costs. The durable writer includes serialization, state/event hashing, pending and stream rows,
and the SQLite `synchronous=FULL` transaction commit; it excludes engine evaluation. Projection
time includes all 13 snapshots, 10 seat observations and committed stream fanout. Controller
time includes scripted selection and pure validation outside those measured commit costs.
These cumulative diagnostics reset on recovery and are available only through the trusted
in-process handle, with no HTTP, WebSocket or MCP tool exposure. They do not alter game state,
RNG, input pins, or the requirement to commit accepted commands before acknowledgment.

Completed CI f16f459 spent about60% of wall time in projections and32% in the durable
writer. Legal-random averaged37.0ms of durable writer work and70.1ms of projection work
per accepted command (3407 total). Engine evaluation was20.336s, writer126.057s,
projections238.672s and controller selection12.562s over397.550s wall time.
The pass campaign measured12.064/61.042/117.679/1.141s in those categories;
HTTP legal-random measured21.569/133.191/255.733/13.347s. Reusing unchanged
perspective and seat projections remains the leading performance follow-up, subject
to privacy and invalidation tests. This is deferred; durability and per-command
acknowledgment stay unchanged.
