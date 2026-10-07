# Campaign watch

The spectator client uses React 19, strict TypeScript, Vite and PixiJS v8 WebGL.
Python 3.11+ and Node.js 22.12+ are required (tested locally on Node 24.12.0).

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
Map layer controls show published routes and boundaries, with explicit survey coverage.

Perspective changes clear previous authorized state and subscribe again. Filtering
happens in the server before delivery (or the development mock transport). The renderer never invents details for
an undisclosed enemy stack. The real server must enforce the same authorization on
every channel; client filtering is not an access-control boundary.

## Campaign access

Open the local board access URL printed by `cna-server`. Its `#cap=` fragment is
captured and removed from the visible URL immediately. The board retains the
capability in memory and this tab's sessionStorage, keyed by backend origin;
credentials for a different port or hostname are never reused. If browser storage
is disabled, the access link still works in memory for that page. Forget access
clears the stored capability and reloads to discard board and transcript state.
Treat access links and the server's capability file as secrets.

Real connections require a capability. Without one, the page displays access
instructions and sends no API requests or campaign sockets. `/api/session` supplies
the initial perspective, campaign binding and operator status before the viewer
mounts. Side and seat access stays in its assigned campaign; forbidden perspectives
and campaign controls are disabled. The server enforces access on all channels.

Every API fetch sends `Authorization: Bearer <capability>` and refuses redirects.
The WebSocket uses `?cap=<capability>` because browser sockets cannot set an
Authorization header. Error messages never echo token-bearing URLs or raw response
bodies. An HTTP 401 unmounts the viewer and clears retained private state and the stored
credential. Scope-denied socket close code 1008 clears retained view/transcript state
and stops retries for that subscription. Network failures still reconnect normally.
Backend URLs are restricted to local HTTP(S) origins (`localhost`, `127.0.0.1`,
`[::1]`) without embedded user information.

For Vite against a separate backend, append `#cap=<capability>` to the existing
`?server=<encoded backend origin>` URL. Real browser integration tests take the
operator credential from `CNA_SMOKE_CAPABILITY`; they seed tab storage without
placing a real credential in navigation URLs or test output.

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
An authenticated operator on a bare production URL lists existing campaigns for selection. In development,
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
# Set CNA_SMOKE_CAPABILITY privately from the server's capability file.
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
8 seconds with the mock stream active and no overlays, for the ordinary 18-unit fixture and the dense 290-unit fixture.

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

A later run with the dense synthetic fixture measured 53.6 FPS (430 frames over
8,016 ms, median 16.7 ms, P95 16.8 ms), with 290 mapped units in 40 stacks and six
unplaced/off-map units on the same published grid. The mock stream remained active.
This is warm navigation under software WebGL; high event throughput, actual CNA
campaign data, high-DPR screens, overlays and hardware GPU browsers still need profiling.

## Dense formations and decision windows

In development, `/?fixture=dense` opens the synthetic stress fixture; add
`&paused=1` to inspect the initial positions before mock events start. This has no
real CNA roster or rules values and is excluded from production. Select a stack to
fan out up to 20 counter sprites in a bounded grid; ordinary stacks show at most
three counters with a count badge. The inspector lists every disclosed member,
scrolls independently and offers a name/id/type search for stacks larger than eight.

The formations panel follows `UnitView.parent`, supports nested expansion and
search, and retains ancestor paths for matched units. Missing parents retain their
published ids; malformed cycles cannot hide units. Selection also works for
server-disclosed units with no hex or an off-map box id. Locations not present in
the map are labelled and inspected without panning to invented coordinates.
Only projected units participate; undisclosed members are never inferred.

The read-only pending panel displays the server's open decision windows, seat,
kind, sequence and `rules` citations. CNA special-unit locations arrive in
`UnitView.detail.location`: `{at: "off_map", id}` or
`{at: "awaiting_setup", group}`, with `hex: null`. The roster groups and inspector
use this published location detail; missing detail is labelled "No map position".

## Actual Graziani browser verification

The CNA check uses `kind: "cna"`, `rules_profile: "cna-2021-dev"`, a zero seed,
`paused: true`, and `controller: "legal_random"`. Start the server with
`CNA_CAMPAIGN_DIR` pointing at a fresh scratch directory; old databases remain
pinned to their original executable/content. Then run:

```powershell
$env:CNA_SMOKE_SERVER='http://127.0.0.1:3000'
# Set CNA_SMOKE_CAPABILITY privately from the server's capability file.
npm run smoke -- cna.spec.ts
```

Verified against server adapter `194c2fd`: 286 disclosed units, of which 211 are
mapped, 53 await setup and 22 occupy off-map boxes; 32 stacks with a largest stack
of 24, and six dump markers. The browser selects the last member of that dense
stack, follows an actual declared OA parent, inspects setup groups and off-map ids,
and displays owning-side dump labels. This run opens the production bundle served
by Rust and receives 18 factual scripted decision entries, with perspective-filtered
seat replay. A second campaign holds a human-controlled initiative window open
and verifies its real rule citations at the top of the formations panel.

These are actual scenario data and transport checks. That historical adapter resolved initiative declarations and skipped unimplemented procedures;
Finished does not mean the full CNA rules are implemented. This check starts no
paid LLM driver and makes no full-campaign throughput claim. Evidence files and
screenshots are saved in the owning scratch folder.

The same production check passed against authenticated server `b140784`, together
with the human-held citation and sandbox control/reconnect checks. Fresh isolated
side and seat browser contexts used actual server-issued capabilities: each
selected its bound campaign/perspective, could not administer or cross campaigns
(HTTP 403), and an attempted operator subscription closed with 1008 before any
stream frames. No-capability API access returned 401. Five real integration checks
passed; this run uses the factual scripted baseline, with no paid model driver.

## Surveyed features, rule cards and movement

All route and hexside feature kinds come from the schema-1 manifest in
`data/map/layers.toml`. Roads are solid, tracks and unfinished routes are dashed,
railroads have ties, pipelines have joints, and slope/escarpment ticks point from
the published high side toward the lower side. These symbols are drawn by our
code. They are not traced game art.

The coverage selector inspects one named layer at a time. A subtle hatch marks
cells or physical internal edges outside that layer's explicit coverage mask.
No feature inside its mask means surveyed absence for that kind. Coverage of
terrain, roads or another feature never implies coverage of a different layer.
The selected hex inspector lists neighbours and their present / surveyed-none /
unknown status, with source citations. Missing neighbours at the map boundary
are identified separately. Feature checkboxes affect drawing, not coverage.
The first audited pilot (`aea0974`) publishes nine road/track edges and explicit
per-kind coverage around C43-C47. Hexside feature records are still empty; their
uncovered boundaries remain unknown. `?layers=fixture` enables clearly labeled
generated feature data in development only, without altering published records.

Citation buttons open registry titles and paraphrased summaries on hover or
keyboard focus; Escape dismisses the card. `tools/export_viewer.mjs` selects
Python 3.11+ (or the executable named by `PYTHON`) and invokes the web-owned
Python exporter before dev, test and build. It exports registry text and runs
the authoritative `tools/rules/coverage.py --json` command. Derived JSON stays
ignored. The coverage panel provides applicable/implemented counts at every
registry timing anchor, plus tested, unsupported and missing totals. Cases
with several timing anchors count in each row; scenario totals count distinct
cases. This is build-time source-citation coverage, including conditional
cases, not evidence that every case runs in the selected rules profile.
Registry and engine hashes identify the export's inputs.

The pending movement panel reads only the published legal action schema's
unit enum. It lists remaining eligible units for the disclosed seat and lets
spectators inspect them; missing schemas remain explicit. Unit movements
animate their disclosed paths from the preceding projected origin, with
constant speed along each leg. Anonymous stack updates/removals pulse their
disclosed location; no enemy route or composition is inferred. Animation does
not delay adjudicated state, supports reduced-motion preferences, and clears
on perspective changes, resyncs and history jumps. Green counter dots and
inspector rows identify units moved in the current segment; snapshot flags
preserve these highlights after reconnects. Up to 128 simultaneous visual
tracks are retained, with at most 4,096 entered hexes per route.

### Layer navigation measurement

Measured 2026-10-07 against viewer `2cf4532` and the audited pilot from
`aea0974`, with all feature switches on, road coverage hatch on,
movement animation enabled, three transcript tabs and the mock stream active.
Chromium 153.0.8010.12, Windows, ANGLE Vulkan SwiftShader software WebGL,
1800 x 1050 viewport, DPR 1. Each case warms for 1.5 seconds, then uses real
pointer-down pan and wheel input for eight seconds. The map layers share the
terrain's culled 10-by-10-hex texture caches.

| Fixture | Hexes | Positive features | FPS | Median / p95 frame ms |
| --- | ---: | ---: | ---: | ---: |
| Published grid, audited road/track pilot | 7,023 | 9 | 60.0 | 16.7 / 16.8 |
| Synthetic terrain and generated layers | 10,000 | 2,961 | 60.0 | 16.7 / 16.7 |
| Dense synthetic roster, real geometry and generated layers | 7,023 | 2,061 | 53.3 | 16.7 / 16.8 |

These measure warm navigation, not full-campaign event throughput or a hardware
GPU. The dense fixture has 290 mapped units in 40 stacks, plus six off-map or
unplaced units. Positive-feature stress data is generated and explicitly
labeled. A separate browser check selected C4419 on the actual pilot and verified
positive roads, surveyed road absence, unknown roads and their source citations
in the same inspector; it saved a screenshot with no browser errors.
`npm run smoke -- layers.spec.ts benchmark.spec.ts` exercises coverage controls
and writes the measurements to the owning scratch folder.

### Actual movement and replay verification

The new movement browser check uses a fresh authenticated CNA development
campaign, hands Axis Front Line to a human controller, and lets the scripted
other seats reach its movement window. It reads the projected legal unit enum,
validates an adjacent move through the rules engine, submits it with the current
controller epoch/revision, and watches the production board. IX Libyan Bn moved
from C4020 to C3920 from a window containing 116 eligible units. The board
animated the move, removed moved units from the remaining list, preserved its
highlight after reload, displayed the registry citation card, and cleared the
private movement panel and animations on an opposing-side switch. This starts
no paid driver.

The full replay browser check now uses `pass_when_possible` so it can verify
stable completion without spending time searching movement orders. It received
90 factual decision transcripts on the movement-enabled development profile;
the separate validated-move check supplies positive movement coverage.
`cna.spec.ts`, `server.spec.ts` and `movement.spec.ts` also cover actual
campaign-bound side/seat authorization and sandbox control/reconnect.

Historical highlights are reconstructed from the contiguous selected segment,
with snapshot and explicit unit flags plus authorized movement events. Repeated
movement cycles separated by combat cannot inherit an older cycle's events.
Movement metadata remains attached to each retained frame even after its
establishing event is evicted. Playback before the 600-frame retention boundary
is unavailable; fresh snapshots remain the reconnect baseline.
