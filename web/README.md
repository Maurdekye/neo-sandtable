# Campaign watch

The spectator client uses React 19, strict TypeScript, Vite and PixiJS v8 WebGL.
Node.js 22.12+ is required (tested locally on Node 24.12.0).

```sh
npm ci
npm run dev
npm run lint
npm test
npm run build
```

Open http://127.0.0.1:5173. Development starts a deterministic synthetic campaign
and three fake AI sessions. The default board consumes the real cartographer grid
from `data/map/hexes.csv` (7,023 canonical hexes) and `aliases.csv`. Published terrain classifications are rendered directly; missing terrain
remains explicitly **unclassified**; units, strengths, combat messages and transcripts
are demonstration data, not a faithful scenario or rule adjudication. No original
art, source scans or rulebook prose are used. `?map=synthetic` switches to an
independent 10,000-hex terrain fixture for development and measurement.

## Controls

Drag the map to pan; wheel to zoom. Select a hex to fan its stack out, then choose
any unit in the inspector list. Formations, objective markers and event cards focus the map. Objective diamonds and labels come from the authorized marker stream. Agent
sessions are tabbed beside the board. Each seat offers kind filters and auto-scroll;
scrolling away from the bottom pauses following, and Follow live resumes it.
Calls and results are paired by `call_id`, including results received first.

Pause playback freezes the viewed frame while live events continue to buffer.
Step, the history slider and Play history navigate retained frames; speed affects
only replay. Return to live follows the newest frame. The HISTORY badge identifies
a past view. Pause campaign is a separate, clearly labelled **mock** operator
control. Live operator controls use the HTTP pause/resume API and show acknowledgements or errors separately from playback. Controls are disabled in history and other perspectives.
Overlays are disabled with a pending-data label until their data exists.

Perspective changes clear previous authorized state and subscribe again. Filtering
happens in the server before delivery (or the development mock transport). The renderer never invents details for
an undisclosed enemy stack. The real server must enforce the same authorization on
every channel; client filtering is not an access-control boundary.

## Stream and ownership

Protocol structures are imported from lead-owned `src/generated/`; Rust
`cna-protocol` generates them. `src/protocol/index.ts` only reexports types and adds
UI aliases. Event sequences are contiguous per perspective and use JSON numbers.
The store requires hello then snapshot, checks subsequent sequences, clears stale
state on gaps and requests a fresh snapshot with `from_seq: null`. A reconnect
with intact state resumes from the last good sequence and applies event replay
after hello, without requiring another snapshot. Unknown event kinds
advance the cursor without applying an unknown transition. Buffers retain at most
600 immutable frames and 1,200 transcript entries; history clamps to the earliest
retained frame on eviction. Transcripts align to event frames via `game_seq`.

The mock generator is dynamically imported only under `import.meta.env.DEV` and is
absent from the production build. To connect a campaign, open
`/?campaign=<id>`. The adapter uses the page origin by default; when using Vite
against a separate backend, add `&server=http://127.0.0.1:<port>` (URL-encode the
server value). It connects to `/api/campaigns/{id}/stream`, using WSS for HTTPS.
A bare production URL lists existing campaigns for selection. In development,
`/?server=<encoded URL>` opens the same campaign chooser; a bare development URL
continues to use the mock fixture.

The adapter guards incoming JSON before rendering and retries failed connections
with a 500 ms to 10 s exponential delay, subscribing with the last good event
sequence. A new socket is opened for every perspective change or resubscription,
so queued messages from an old projection cannot populate the new view. Hello
must acknowledge the requested perspective before the snapshot or resume replay; unsupported
protocol versions and malformed payloads produce a visible retry status.

Transport tests use injected sockets and a browser WebSocket fixture. The real
sandbox check connects to `cna-server`, verifies the nine-unit snapshot, three
objectives, live events, perspective filtering, campaign discovery and HTTP
pause/resume. Run it against a server from this checkout:

```powershell
$env:CNA_SMOKE_SERVER='http://127.0.0.1:3000'
npm run smoke -- server.spec.ts
```

Campaign lifecycle status refreshes every five seconds through the projected
HTTP inspection API. The server emits truthful scripted decision entries through its persisted
transcript channel; browser checks verify live display and side filtering. Assistant
text, tool pairing and System 1 entries are verified through mock and WebSocket
fixtures; this smoke does not run a paid LLM driver. The sandbox is synthetic and is not faithful CNA adjudication.

## Renderer and measurement

Static terrain is grouped into 10×10 axial chunks, culled by viewport, and cached
as textures at zoom-band resolutions 0.5, 1 and 2. Original NATO-style counter SVGs
are rasterized to shared textures, then rendered as batched sprites. Retired
counter texture variants are bounded at 256 plus currently displayed variants.
An FPS badge measures actual Pixi ticker frames, not an estimated score.

Reproduce browser checks and the navigation benchmark:

```sh
npx playwright install chromium
npm run smoke
# or just navigation timing:
npm run smoke -- benchmark.spec.ts
```

Playwright owns and shuts down its local Vite process. Screenshots and benchmark
JSON go two directories above web, in the owning agent's scratch folder. Browser
checks cover the inspector, pairing, System 1 sessions, perspective replacement,
paused buffering, stepping, return to live, event focus and pan/zoom. The benchmark
warms the caches, pans on every animation frame, zooms every 30 frames, and samples
8 seconds with the mock stream and 18 counters active, with no overlays.

Measurement results are recorded below after running the benchmark. These are
headless software-WebGL observations; they are not a hardware-GPU performance
guarantee or a full-campaign profile.

### Observed navigation run (2026-10-06)

Windows 11, AMD Ryzen 7 9700X, Node 24.12.0, Chromium 153.0.8010.12.
1800×1050 CSS pixels, DPR 1; ANGLE Vulkan **SwiftShader software renderer**,
not the installed RTX 3080 Ti GPU. Eight seconds each, warm-cache pan plus zoom.

| Grid                   | Frames |  FPS | Median frame | P95 frame |
| ---------------------- | -----: | ---: | -----------: | --------: |
| Real 7,023 hexes       |    480 | 59.9 |      16.7 ms |   16.8 ms |
| Synthetic 10,000 hexes |    437 | 54.5 |      16.7 ms |   16.8 ms |

This measures navigation with the current 18-unit fixture. Dense full-campaign
counters, high-DPR screens, overlays and ordinary hardware-GPU browsers still
need profiling when their actual data is available.
