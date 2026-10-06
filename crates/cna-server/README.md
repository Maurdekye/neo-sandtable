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

Creation accepts JSON with `kind` (`sandbox`, the default, or `cna`), `rules_profile`, `seed`
(32 byte values), optional `title` (default "Campaign"), `paused` (default false), and `controller`.
Use `paused: true` to attach viewers before starting a fast campaign. No LLM driver or paid
provider is started by the server.

- `kind: "sandbox"`, `rules_profile: "sandbox-v1"` runs the **synthetic game**. Controllers:
  `legal_random`, `pass_when_possible`, `aggressive` / `scripted:aggressive`, or `human`.
- `kind: "cna"`, `rules_profile: "cna-2021-dev"` runs the real Graziani's Offensive setup and
  sequence through the development ruleset. Currently it resolves initiative declarations and
  skips unimplemented steps; reaching Finished does not mean all CNA rules are implemented.
  Controllers: `legal_random`, `pass_when_possible`, or `human`.
- `kind: "cna"`, `rules_profile: "cna-2021-full"` uses the strict ruleset and stops visibly at the
  first unsupported applicable procedure. It never substitutes an order. Aggressive is available
  only for the sandbox (including handover). Unknown kinds, profiles and mismatched pairs are rejected.

For example, POST `/api/campaigns` with `{"kind":"cna","rules_profile":"cna-2021-dev",
"seed":[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],"paused":true,"controller":"legal_random"}`. Rust callers supply
`CreateRequest.kind: CampaignKind::Sandbox` or `CampaignKind::Cna`; legacy JSON without `kind`
still creates a sandbox. `campaigns::create` dispatches both kinds; `sandbox::create` remains the
sandbox-specific factory.

| HTTP endpoint | Purpose |
|---|---|
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
metadata query before opening a writer. Its input fingerprint includes the map CSVs, the TOMLs
actually read by the unit and Graziani setup loaders, every table TOML, and the `land`, `airlog`
and `scen` registry folders. Unread files and notes do not affect it. Paths and bytes are hashed
in sorted order; a change during loading is rejected. The CNA engine fingerprint is embedded at
build time from the core, protocol, content, tables and rules Rust sources, their manifests and
Cargo.lock. It is conservative: rebuilding with changed inputs requires matching saved pins;
there is no migration or implicit profile fallback. Existing sandbox pinning remains unchanged.
The real-scenario tests cover both baselines reaching Finished, private transcript replay,
checkpoint plus tail recovery, strict-profile rollback, HTTP creation/control, input drift and
mixed sandbox/CNA recovery. No provider invocation is needed for these tests.
