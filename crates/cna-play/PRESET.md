# Graziani ten-seat runbook

The preset configures all ten logical seats with the dated model
`claude-haiku-4-5-20251001` under `cna-2021-dev`. It parks and resumes the same
native session per seat, with at most two active CLI processes. Schema-forced
answers run locally before taking a process slot or admitting a model turn.

**Preparation checkpoint:** real model admission is closed. The Claude driver
currently supplies no verified estimate bound, so this command pauses cleanly
before launching any native process. The pinned native price/output enforcement
proof, production-board evidence for the complete inert preset and owner spend approval remain required.
The complete inert game-turn below exercises the launcher; it does not prove native enforcement or predict AI play.

After those gates are satisfied and the owner approves the spend, the proposed
command for one game-turn is:

```powershell
$env:CNA_LIVE_CLI_TESTS = '1'
cargo run -p cna-play -- --preset graziani-haiku --stop-after-turn 1 --budget-usd 30
```

The USD30 value is an example allocation, not an approved spend or measured game-turn
cost. It assigns USD3 to each seat by default. To stop after an Operations Stage,
add `--stop-after-opstage 1` with the chosen game-turn. The persisted server fence
previews each Advance and discards a transition beyond the boundary. Bindings stay
healthy and the last committed game/RNG are retained. Decisions already in flight
may finish and reconcile usage while the campaign is paused.

Resume the printed campaign database with an explicit raised boundary:

```powershell
cargo run -p cna-play -- --resume <campaign.sqlite> --stop-after-turn 2
```

Resume retains native session IDs, lifetime usage and allocations. Raising the
boundary records controls while paused; starting the supervisor explicitly resumes.
Optional `--seat-budget-usd SEAT=USD` entries must cover every model seat, total no
more than the existing global cap, and use the same budget unit. Rebalance never
redistributes automatically or resets spend. An exhausted seat pauses the campaign;
rebalance is refused while a model turn remains admitted or unresolved. An unknown
spend stop needs accounting reconciliation, not merely a new boundary or more funds.

Each decision has a 60-second timeout; lifetime turn, tool and wall limits also
persist. Their exhaustion causes a clean controlled stop. A provider error can still
fail its binding. CLI resume failure may reseed from the seat's durable notebook
within its existing recovery allowance.

## Estimate accounting and admission

A provider-reported cost limit is an estimate, can overshoot by at most about one
model turn, and is not a guaranteed billing cap. On `claude-5`, reported USD is an
API-equivalent estimate, not subscription charges; session and weekly plan windows
also constrain availability. No real run is approved by this runbook.

The launcher requires Claude Code >=2.1.277 at runtime. It uses the latest cumulative
`total_cost_usd` result for each session and keeps lifetime deltas across reseeds,
never summing cumulative results. Every spawn/resume receives only its seat's
remaining allotment through `--max-budget-usd`. This print-mode client estimate
backstop excludes restored spend, so the persistent journal remains necessary.
Unknown cost basis, missing results or crash uncertainty stop admission. Result
usage, cache channels and reasoning remain separate in canonical usage snapshots.

Admission requires a proved pinned-model response ceiling, not the largest observed
turn. For Haiku the dated official table gives a full context bound of 200000 and a
model maximum output of 64000. Subscription one-hour cache writes supply the worst
input price, USD2 per million tokens; output is USD5 per million. The conservative
estimate ceiling is therefore `200000*2/1000000 + 64000*5/1000000 = USD0.72`.
A lower output setting is eligible only after installed seat requests prove it;
`CLAUDE_CODE_MAX_OUTPUT_TOKENS` applies to most requests and is not sufficient proof
by itself. Native refusal fallback is disabled and no fallback model is passed.

A native model turn can make multiple provider responses around tool calls. The
whole-turn envelope is its actual native remaining-seat cap plus one proved response
ceiling for the response crossing that cap. Atomic admission reserves that envelope;
unreserved global funds must also cover a separate one-response margin (minimum
USD0.25). The seat itself must have at least a response ceiling plus margin remaining.
Accounted outcomes settle reservations; interrupted or uncertain work retains its
full envelope and blocks admission across crashes. Observed costs never determine
admission. This envelope requires proof of native cap enforcement for the installed
invocation; real admission stays closed until that proof exists.

The model/price table is dated2026-10-07 and cites official sources fetched by the
project lead: [CLI reference](https://code.claude.com/docs/en/cli-reference),
[cost tracking](https://code.claude.com/docs/en/agent-sdk/cost-tracking),
[environment variables](https://code.claude.com/docs/en/env-vars),
[model overview](https://platform.claude.com/docs/en/about-claude/models/overview),
[pricing](https://platform.claude.com/docs/en/about-claude/pricing).
Bundled CLI price estimates can drift from billing; provider Usage/Cost APIs or the
Console are billing authority. The model source gives an earliest retirement boundary
of2026-10-15, not a confirmed retirement date. An unavailable dated model stops;
alias substitution is not automatic.

`--budget-tokens N` is an alternative persistent unit for a driver that proves native
token enforcement. Claude has no such proof and rejects token-mode admission.
No guessed dollar conversion or soft local token counter supplies that proof.

## Offline evidence

```powershell
$env:CNA_LIVE_CLI_TESTS = '0'
cargo test -p cna-play --test preset -- --nocapture
```

These tests call only each inert seat's scoped HTTP MCP tools against the real server.
Sandbox proves two slots, same-session parking, persisted usage, healthy boundary
stops and recovery into a raised boundary. Graziani covers a bounded setup slice of
at most two fake admissions per seat; its per-seat/phase counts and prompt/observation
byte sizes are printed as `OFFLINE_PRESET_SAMPLE`. Fake usage and injected ceilings
are synthetic, never provider or billing measurements. The final test uses a real
Claude driver with an absent executable to prove the unverified-bound gate stops
before any native start. Native request enforcement and the complete preset production-board proof remain pending. Seat tokens are private; board links printed by the
trusted launcher must not be copied into public logs or fixtures.


## Complete inert game-turn and browser handoff

The following runs the exact runbook preset arguments with in-process fake drivers,
through the real server and each seat's own MCP endpoint. It never launches a native
CLI or calls a provider. The fake passes whenever offered, allocates the advertised
maximum first-line truck quantities during setup, and otherwise chooses minimal
enumerated mandatory answers. Validation remains required; an illegal answer fails.

```powershell
$env:CNA_LIVE_CLI_TESTS = '0'
$env:CNA_PRESET_FULL_OUTPUT = '<absolute private report path>'
cargo test -p cna-play --test preset full_game_turn_inert_preset_reports_each_seat_and_phase -- --ignored --exact --nocapture
```

On the f83abf7 baseline the proof completed in 174.86 seconds on the local machine.
It reached all three Operations Stages and the persisted GT1 fence with healthy
bindings and no budget stop. Re-previewing that fence changed no projected state or
event sequence. The saved clock remains at OP3: crossing automatic Post and GT2 in
one Advance discards that entire preview. An `Ok` supervisor return alone never
counts as a completed game-turn. Counts reconcile against every canonical
`DecisionResolved`, including locally forced answers.

| Seat | Setup fake windows | OP1 | OP2 | OP3 | Other pre-stage | Local forced |
|---|---:|---:|---:|---:|---:|---:|
| Axis commander | 19 | 1 | 1 | 1 | 0 | 3 |
| Axis front line | 0 | 9 | 9 | 8 | 0 | 13 |
| Axis rear area | 0 | 7 | 7 | 7 | 0 | 15 |
| Axis logistics | 132 | 3 | 3 | 3 | 1 | 42 |
| Axis air | 55 | 1 | 1 | 1 | 1 | 3 |
| Commonwealth commander | 28 | 0 | 0 | 1 | 0 | 2 |
| Commonwealth front line | 0 | 9 | 9 | 9 | 0 | 12 |
| Commonwealth rear area | 0 | 2 | 2 | 2 | 0 | 27 |
| Commonwealth logistics | 205 | 3 | 3 | 4 | 1 | 41 |
| Commonwealth air | 41 | 1 | 1 | 1 | 1 | 3 |

This policy produces 593 fake windows and 161 local forced answers, 754 canonical
answers total. Setup accounts for 480 fake windows; OP1-3 account for 109, and other
pre-stage windows for four. Peak fake concurrency is two. These are measurements of
this policy, not required model-call counts for a supplied or moving army. Setup is
reported separately and changes when the chosen allocations change.

The report records UTF-8 sizes of system/user prompts, observations, action
schemas and tool-result text, and each window's anchor. These are actual bytes,
not token counts; persistent history, multiple model responses and provider cache
accounting make byte-to-token or byte-to-dollar conversion unmeasured. Fake usage
snapshots deliberately contain synthetic 10 input tokens, 5 output tokens and
USD0.01 per completed window. The board must label evidence as synthetic. The
older two-window Haiku sample in README measured USD0.1009509 and 6225 raw input /
6400 output tokens under its restricted-supply conditions. Applying that unit cost
to setup, this pass policy or an active full stage would not be a measured estimate.

For the isolated production-browser proof, set `CNA_PRESET_BOARD_READY` and
`CNA_PRESET_BOARD_RELEASE` to fresh absolute paths in trusted helper scratch before
running the same command. The ready JSON initially has `state: "ready"`,
`campaign_id`, `board_url` and `usage_basis`. The operator URL is a same-origin
fragment capability; read it into helper memory only. Never copy it into a fixture,
public log, screenshot or a seat's environment. The file is trusted output, not a
seat working directory. It is updated after the game stops: successful completion
uses `state: "OFFLINE_PRESET_GT1_BOUNDARY"`, `complete: true`, `final_clock` and
`fence_repreview: true`. The server remains alive while the helper captures board
transcripts, synthetic usage and the paused boundary. Create the release file only
after capturing evidence; its contents are ignored. A separate 120-second browser
acknowledgement guard then shuts the helper down and fails if acknowledgement was
missing. Omit both variables for a pure server measurement. The Rust producer removes
both fresh handoff files after server shutdown on success, failure or missing
acknowledgement, and reports cleanup errors. An unwinding guard also attempts removal.
The browser helper retains its own `finally` cleanup as a second cleanup path.

The new ignored-test hang guard is provisional, based on the measured local run;
recalibrate it to about twice its first completed CI duration. Default tests and
production model, tool and wall budgets are unchanged.
