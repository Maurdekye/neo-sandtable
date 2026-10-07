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

| Fixture                                                    |  Hexes | Positive features |  FPS | Median / p95 frame ms |
| ---------------------------------------------------------- | -----: | ----------------: | ---: | --------------------: |
| Published grid, audited road/track pilot                   |  7,023 |                 9 | 60.0 |           16.7 / 16.8 |
| Synthetic terrain and generated layers                     | 10,000 |             2,961 | 60.0 |           16.7 / 16.7 |
| Dense synthetic roster, real geometry and generated layers |  7,023 |             2,061 | 53.3 |           16.7 / 16.8 |

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

## Blind set-up placement

The pending panel lists the projected units awaiting set-up. A highlighted unit
button identifies the current placement request through the server's
`space["x-context"].unit` and `.group` metadata. Older projections without context
show an explicit association-unavailable message; the viewer never derives ids
from the request's prose or its opaque id. Units without a disclosed open request
remain inspectable and say that no placement window was supplied.

Each unit or dump placement uses the published string-enum legal destination
choices. The selected request highlights its mapped hexes in blue. Show legal area
fits the map to that domain; searchable destination lists focus individual hexes.
Off-map destinations stay labelled and cannot move the map to invented geometry.
Missing action schemas do not become an empty legal domain. The highlight uses
culled chunks and includes at most the authorized published choices.

Set-up choices are blind: the board does not preview accepted private choices or
infer them from transcripts. Unit positions remain at awaiting-set-up until the
server publishes them at shared-window closure. Disclosed awaiting-to-map unit
updates and placed markers pulse into view; anonymous opponent stacks gain only
the published presence glyph. Perspective changes and history navigation use only
the selected authorized projection. Reduced-motion controls also apply to these
placement effects.

`npm run smoke -- setup.spec.ts` exercises the real Graziani set-up with an
operator and an isolated opposing-side capability. It requires the set-up context
metadata and a fresh server database, as for the other actual integration checks.
It holds a human placement window, validates/submits one legal choice, lets
scripted seats finish set-up and verifies the final public presence separately
from private unit identities. It starts no paid model driver.

The checked real set-up run on engine `f5542bd` selected 8th Field Artillery Regt
from its five published Cairo destinations. A validated E1730 choice remained
buffered: an isolated Axis viewer received neither its position nor its identity.
Server-scripted land placements pulsed into the board at shared-window closure;
the unit inspector then showed E1730, while the opposing stack disclosed only
presence. Both front-line seats were held to prevent later combat updates from
masking a missing closure publication. Initial air allocation, which has no board
projection, used bounded fixture answers from its exact published enum. The run
took 196.7 seconds on the busy shared machine and reported no browser errors.
This is a correctness check, not a navigation FPS measurement.

## Coast movement and combat

The feed retains authorized event-envelope locations through playback. Located
notes, dice and removals have a Locate control; an absent location remains
unlocated. The viewer does not turn prose or a former unit position into a
locator. Dice show the published reading and a registry citation card. Combat
results retain their summary and optional structured detail, and removals show
their reason. Movement and notes can be filtered together so private development
stops remain visible alongside the adjudicated route.

Movement follows each disclosed route leg, with the event's CP cost beside the
animated counter. Own counters and inspectors show only supplied status: engaged,
pinned, reserve, gun position, moved this segment and broken vehicles. Quarter CP
values are converted to points for display; raw detail remains inspectable.
Opponent stacks disclose presence without these private badges.

The optional terrain classification lens uses mint fill for classified cells,
hatching for unclassified cells and a blue outline for corridor membership. The
corridor is a digitization priority, not a movement rule or coverage guarantee.
Reviewed strip buttons use data/map/strips.toml, including explicit unresolved
pipeline and control halo limits. Build-time export consumes that manifest and
the corridor membership without rewriting game data.

`npm run smoke -- coast-view.spec.ts` tests this presentation with a clearly
synthetic transport, including scope clearing. `coast.spec.ts` is the separate
real server check: legal_random seats and an actual classified route crossing a
published road-spine pair, no paid driver. It can also resume its own paused
smoke campaign without changing the engine binary. It reports
observed event counts and missing locations; rendering a synthetic combat event
is not evidence that the baseline fought a combat in the real run.

The checked coast run on engine `9fad9fa` used a fresh Graziani campaign. Its
first bounded three-minute attempt reached logistics; a second attempt resumed
that same paused campaign and verified reviewed-strip movement in 58.4 seconds.
Cirene I/158 Infantry Bn crossed a road-spine pair from its disclosed prior
position, then continued through C4020 to classified C3921, beyond that strip.
The published movement event cost was 3 CP. The separate current unit inspector
reported cumulative CP. Scope switching removed its private identity and active
motions. No browser errors or CombatResolved events were observed. Among 540
actual events, 111 Notes and one die event had no authorized location; the viewer
kept them unlocated rather than deriving positions from their prose.

With terrain classification and corridor enabled, an eight-second navigation
sample in Chromium 153.0.8010.12, Windows, SwiftShader software WebGL, 1800 by
1050 and DPR 1 measured:

| Scene                                                      |  FPS | p95 frame interval |
| ---------------------------------------------------------- | ---: | -----------------: |
| Real 7,023 hexes, 11 surveyed features, mock stream        | 60.0 |            16.8 ms |
| Synthetic 10,000 hexes, 2,961 generated features           | 41.8 |            50.0 ms |
| Real grid, 2,061 generated features, 290 units / 40 stacks | 41.5 |            49.9 ms |

All feature kinds, road coverage hatch, classification lens and transcripts were
on. These are short navigation samples on a shared machine, not a hardware-GPU
claim or full-campaign throughput measurement. The dense synthetic scene
misses 60 FPS with these overlays enabled. Raw measurements live in the scratch
artifact `navigation-benchmark-coast.json`. Reproduce with
`CNA_BENCHMARK_COVERAGE=1 npm run smoke -- benchmark.spec.ts`.

## Watching longer campaigns

The collapsed Stage overview groups only received events by game turn and OpStage.
Movement groups use the unit's disclosed parent formation, or the unit itself when
no parent is supplied. Each group shows its received movement count and largest
published path in route hexes. The entry opens that exact movement frame and focuses
its published destination. Anonymous opposing stacks contribute presence entries;
they do not become invented units, routes or movement counts.

Combat result totals count CombatResolved events. Dice, narrative combat notes,
reasoned removals, breakdown counters, arrivals and off-map transitions remain
separate entries. Supply and shortage headings classify received Notes; they do
not establish numerical losses or infer supplies from prose. Generic Locate uses
only the supplied event locator, while intrinsic movement/combat/stack positions
remain usable. An absent location stays unlocated.

Memory is bounded to the latest 12 stage summaries, 80 notable entries and 128
formation groups per stage, plus 256 accepted commentary checkpoints. Received
event counts remain cumulative after an entry's detail is omitted. A resumed
snapshot marks its stage partial: earlier events were not received. A perspective,
campaign or resync reset clears summaries. The ordinary playback ring retains
600 frames. Summary entries retain their own exact immutable frame beyond that
ring; a checkpoint caption explains that intermediate frames are unavailable and
Step/Play are disabled until returning to retained history or live play. No state
is reconstructed from missing events.

Seat commentary comes exclusively from canonical accepted
DecisionResolved.explanation. It is shown with that seat's decision in the overview
and paired to its submitted transcript by seat, decision id and game sequence.
The server's seat audience admits the owning seat, its side and the operator.
Absent explanation means no commentary. Rendering uses React text escaping:
HTML, Markdown and apparent URLs remain literal text, without links. Scripted
baselines currently supply no explanation. The synthetic browser regression
checks hostile literal text, perspective reset and archival playback after 620
events; the real baseline proof verifies movement summaries without invented
commentary. Optional nullable schema fields do not change required movement unit
enum extraction.

Actual baseline proof on 2026-10-07 (viewer f1b56e4 over map/engine699a3ea):
a fresh zero-seed Graziani campaign reached scripted setup within the first
three-minute bound. Resuming that same saved campaign and binary completed the
browser check in 10.2 seconds. Its partial OpStage1 summary received 55 new events,
retained two movement groups, and jumped exactly to event2163 with LocateC3921.
Cirene's received route was C4120/C4020/C3921 (3CP), crossing road-spine-0001 before
leaving it; another received path was C4120/C4220. The legal_random dev profile
still has incomplete-map assumptions; the demonstration does not certify
full-profile route legality. Opponent switching removed private movement and
commentary, with no browser errors or fabricated baseline explanation. No actual
CombatResolved was received in this short run. Screenshot and machine-readable
proof are board-graziani-stage-timeline.png and
stage-timeline-browser-verification.json in board scratch. Canonical commentary
has separate hostile-literal fixture coverage; this baseline proof does not claim
an AI supplied an explanation or cover a whole multi-hour game.

## Operator seat monitoring

The live operator strip shows all seats, their literal controller labels, pending work,
and the latest received acceptance and canonical explanation. Commentary remains plain
text. Failed tool results are labelled separately from a paused controller. Existing
operator seat/observe endpoints refresh pause reasons and controller epochs every ten
seconds after a serial polling pass. A changed epoch means an observed handover; no
past handover is inferred on attachment. Poll failures are visible, and polling stops
outside the live operator perspective.

Waiting time starts when this viewer first receives a pending decision. It does not
claim the decision's actual opening time. Answer counts persist beyond the 600-frame
ring and start at the current snapshot. The recent rate measures received acceptances
over at most sixty seconds, including replay; it is unavailable for the first ten
seconds. Resync and perspective changes clear these observations. Binding monitoring
is hidden during playback because binding polls do not describe historical frames.

Scripted controllers display `scripted, no usage`. Other controllers show missing
tokens and dollars as `not reported`; human-readable lifecycle or usage prose is never
parsed into accounting. Provider and model remain literal controller labels until a
typed report supplies them. No token price is inferred.

The no-paid-call browser proof in `e2e/monitoring.spec.ts` covers a fresh scripted
Graziani campaign and ten synthetic seats with waiting, errors, paused status, observed
handover and escaped hostile commentary. Screenshots and a JSON proof are written to
the board agent's scratch folder; this is a short observation, not a paid ten-model run.

Typed `usage_snapshot` reports replace totals by increasing revision within a controller
epoch; later epochs replace earlier ones and replay never adds totals. A snapshot rebuild
recovers the highest report from retained authorized transcripts. The operator card hides
old-epoch usage after a polled handover. Input and output, cache read and creation, and
reasoning remain separate provider-reported channels. They are never universally added.
Reported dollars use the provider's cumulative figure, without price estimates. Nullable
channels remain `not reported`, including missing USD; known zero remains zero. Reports
with incomplete turns carry an explicit incomplete label. A transcript usage filter exposes
all reported fields, provider/model, epoch/revision and attempt/completion counts to each
authorized perspective. Fixture reports are synthetic and make no paid-call claim.

## Human seat console

`/console.html?campaign=<id>&seat=<seat>#cap=<seat-capability>` is a separate seat-driver
entry. Use the launcher's seat link. The capability stays in memory, is immediately removed
from the fragment, and is never loaded from or written to browser storage. Reloading needs a
fresh link. The console uses its serving origin and accepts only a matching campaign-bound
seat session, never operator or side access. Each tab represents exactly one seat.

Forms consume the decision's advertised schema: labelled choices, bounded numbers and text,
records, optional fields with Omit, and ordered lists with add/remove/reorder. Pass and Done
choices are visible. An optional, default-off checkbox can answer a structurally pass-only
decision; acceptance is logged. Rule references use the existing hover cards. Unit inspection
shows the own seat's engine-provided details, including CP/fuel where available.

Map highlights contain finite advertised enum targets only. Non-enumerated hex/path picks
are candidates whose legality still requires engine preflight. Preflight evaluates the draft
without applying it; submit then rechecks the decision revision and controller epoch. Draft
fields that remain valid survive a revision change; removed enum values become unselected.
Retries of the same intent reuse the idempotency key. Commentary is optional plain text,
limited to 2000 characters, and travels with the accepted decision to its authorized audience.

The console pins the human controller epoch observed at attach. A subsequent handover stops
submissions and shows lost control. A preflight still in flight is cancelled before submit if
control, the decision or the selected form changes. Server epoch/revision enforcement remains
authoritative. The console calls only session and its own seat observe/actions/inspect/validate/
submit routes and subscribes to its seat stream. The trusted integration harness creates and
binds the campaign outside the browser pages; operator credentials never enter those pages.

Console checks: `npm run smoke -- e2e/console.spec.ts` covers schema forms, revision retention,
preflight/handover races, literal commentary, memory-only access and route isolation.
`e2e/console-real.spec.ts` requires a fresh authenticated own smoke server and proves two
separate human tabs: commander setup and frontline coast movement, with cross-seat denials.
`tools/sample_console_schemas.mjs` is an authoring tool outside the console bundle; it samples
advertised action spaces from an all-scripted real Graziani stream, with no paid calls.
