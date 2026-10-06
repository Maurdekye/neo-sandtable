# Live stream protocol (provisional)

> **Status: provisional.** These shapes let the board and the server be built in parallel. Once
> `cna-server` exists, the authoritative types are generated from Rust (`ts-rs`) into
> `web/src/generated/`, and this document is updated to match them. Field names here are the
> intended ones; change them only through the lead.

The server is local (`http://127.0.0.1:<port>`). Commands and queries use HTTP; each viewer opens
one WebSocket for the live stream. All messages are JSON objects with a `type` field.

## 1. Perspectives

Every subscription has a **perspective**, and the server filters everything it sends (snapshot,
events, transcripts, errors) for that perspective:

| Perspective | Sees |
|---|---|
| `operator` | Everything (omniscient). Always labelled as such in the UI. |
| `side:axis` / `side:commonwealth` | What that side may know under `land:3.6` Limited Intelligence, plus its own seats' transcripts. |
| `seat:<seat_id>` | What that seat may know, plus its own transcript. |

Seat ids: `<side>.<role>`, where side is `axis` or `commonwealth` and role is one of `commander`,
`front_line`, `rear_area`, `logistics`, `air`. Example: `axis.logistics`.

## 2. Client → server (WebSocket)

```jsonc
{ "type": "subscribe", "perspective": "operator", "from_seq": null }   // null = start from a fresh snapshot
{ "type": "subscribe", "perspective": "side:axis", "from_seq": 1234 }  // resume after event 1234
```

Campaign control (pause/resume campaign, seat handover, …) goes over HTTP (`POST
/api/campaigns/{id}/…`), never over the stream.

## 3. Server → client (WebSocket)

### `hello`
```jsonc
{ "type": "hello", "protocol": 1, "campaign": CampaignMeta, "perspective": "operator" }
```

### `snapshot` — the full projected state at event sequence `seq`
```jsonc
{ "type": "snapshot", "seq": 1234, "view": ViewState }
```

### `event` — one game event after the snapshot, strictly increasing `seq`

**Sequence numbers are per perspective.** Each perspective has its own contiguous event sequence
(1, 2, 3, …) containing only the events it may see. A global counter with gaps would let a side
infer how much hidden enemy activity happened, which `land:3.6` forbids. Snapshots and `from_seq`
refer to the same per-perspective sequence. All sequence numbers are JSON numbers (typed `number`
in TypeScript) and stay well below 2^53.
```jsonc
{ "type": "event", "seq": 1235, "clock": Clock, "event": GameEvent }
```
A client that sees a gap (`seq` ≠ last + 1) discards its state and resubscribes with `from_seq`
of its last good event (or `null`). The server may also send `{ "type": "resync" }` to ask for that.

### `transcript` — a live entry from an AI seat's session (owner requirement, 2026-10-06)
```jsonc
{
  "type": "transcript",
  "seat": "axis.commander",
  "tseq": 87,                       // per-seat transcript sequence, strictly increasing
  "at": "2026-10-06T10:32:28.693Z", // wall-clock time of capture (display only)
  "game_seq": 1235,                 // latest game event seq when captured (for replay alignment)
  "entry": TranscriptEntry
}
```
```jsonc
// TranscriptEntry
{ "kind": "assistant_text", "text": "…" }
{ "kind": "reasoning",      "text": "…" }            // only if the CLI exposes it
{ "kind": "tool_call",      "call_id": "c12", "tool": "describe_actions", "args": { … } }
{ "kind": "tool_result",    "call_id": "c12", "ok": true, "summary": "…", "detail": { … } }
{ "kind": "decision_submitted", "decision_id": "d-…", "summary": "…" }
{ "kind": "system",         "text": "session started (claude-code 2.1.289, …)" }
{ "kind": "system1_query",  "question": "…", "options": [ … ] }   // System 1 seats
{ "kind": "system1_answer", "choice": "…", "scores": { … } }
```
Transcripts are a separate channel from game events: they never affect adjudication. They are
persisted, so playback can show them alongside the board.

## 4. Core shapes (provisional)

```ts
type Side = "axis" | "commonwealth";

interface CampaignMeta {
  id: string; scenario_id: string; rules_profile: string; title: string;
  seats: SeatInfo[];
}

interface SeatInfo {
  id: string;                 // "axis.commander"
  side: Side; role: string;
  controller: { kind: "scripted" | "llm-cli" | "system1" | "human"; label: string } | null;
  status: "idle" | "deciding" | "paused" | "failed";
}

interface Clock {
  game_turn: number;          // 1..111
  date: string;               // game date of the turn's first day, e.g. "1940-09-15"
  stage: string;              // timing anchor, e.g. "opstage"
  op_stage: 1 | 2 | 3 | null;
  phase: string;              // e.g. "movement_and_combat"
  segment: string | null;     // e.g. "combat"
  step: string | null;        // e.g. "barrage"
  phasing: Side | null;       // Player A/B resolved to a side
}

interface ViewState {
  clock: Clock;
  stacks: Stack[];            // every visible stack on the map
  units: Record<string, UnitView>;   // only units this perspective may know about in detail
  markers: Marker[];          // supply dumps, fortifications, minefields (if visible), …
  pending: PendingDecision[]; // open decision windows visible to this perspective
}

interface Stack {
  hex: string;                // printed id, e.g. "C4218"
  side: Side;
  unit_ids: string[];         // ids the perspective may see; may be empty for an enemy stack
  visible_count: number | null; // null when even the count is hidden
}

interface UnitView {
  id: string; side: Side; name: string;
  kind: string;               // e.g. "infantry", "armor", "artillery", "hq", "truck", …
  size: string;               // NATO echelon: "company" | "battalion" | "regiment" | "brigade" | "division" | …
  nationality: string;        // "italian" | "british" | "australian" | "indian" | "new_zealand" | …
  hex: string | null;
  parent: string | null;      // organization (assigned/attached) for the inspector tree
  detail: Record<string, unknown> | null;  // CPA, TOE strength, supplies, … when authorized
}

interface PendingDecision {
  id: string; seat: string; kind: string; summary: string; opened_seq: number;
}
```

`GameEvent` is a tagged union (`{ "kind": "unit_moved", … }`); the initial set will be defined
with the engine. The board must ignore kinds it does not know rather than fail.
