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

Only `sandbox-v1` is implemented: a **synthetic game, not CNA**. Creation accepts JSON with
`rules_profile`, `seed` (32 byte values), optional `title`, `paused` (default false) and
`controller` (`legal_random`, `pass_when_possible`, `aggressive` / `scripted:aggressive`, or
`human`). Use `paused: true` to attach viewers before starting a fast campaign. No LLM driver or
paid provider is started by the server.

| HTTP endpoint | Purpose |
|---|---|
| `GET/POST /api/campaigns` | List metadata / create a sandbox campaign |
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
handover. `mark_failure(seat, reason)` durably pauses a failed controller, retaining the reason.
`handover` installs a controller and a new epoch and clears its pause. `shutdown` joins the
writer thread. Submit receipts are stable acknowledgements; observations are fetched separately.
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
