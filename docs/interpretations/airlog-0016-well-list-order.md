# airlog-0016 ? Ordering closed well-operation lists

- **Cases:** land:7.11, airlog:48.0, airlog:52.13, airlog:52.16, airlog:52.17
- **Status:** proposed
- **Profile version:** cna-2021-dev; cna-2021-full
- **Decided by:** rules-airlog, with neo-sandtable approval, 2026-10-07
- **Owner review:** pending batch 2

## Question
Both sides can submit private well operations during organization. Operations at the same source can interact: an earlier poisoning or depletion changes a later draw. The source does not specify ordering for simultaneous submitted lists.

## Evidence
Player A and Player B are publicly declared for an OpStage (7.11). The logistics sequence includes water distribution (48.0), and the well rules specify individual draws and attempts (52.13, 52.16, 52.17). Those cases do not define simultaneous list adjudication.

## Ruling
Validate each side's entire list using its own units, stocks, known conditions and public facts. Reserve its CP without rolling or consulting undiscovered opposing well conditions. Once both current windows close, resolve Player A's operations first, then Player B's, preserving submitted order within each list. This is an explicit convention where the source is silent.

If a phase lacks a declared Player A, select the two-side order once with the campaign RNG for that closed operation round. Record that bookkeeping roll privately for the operator. Actual well dice and quantities remain owner-only; conditions become public only when the well rules require discovery.

A list may have one unresolved draw per unit and one attempt of a given operation per well. Further attempts wait for the previous result, so a failed poisoning does not invalidate an already accepted second instruction. Draws by different units may share a well; a later zero-yield draw remains an accepted, charged attempt.

After resolution, each side allocates all of its recorded draws in one atomic list. Pass finishes the step and leaves unused water at the source.

## Rationale
The public OpStage order gives each side the same structural opportunity as initiative changes. Submission order and server validation cannot expose hidden well conditions or determine the outcome.

## Affected behaviour and tests
The batched water procedure stores closed lists in checkpoints and resolves them through finish_step. Tests compare clean versus secretly poisoned wells at answer time, and opposite submission orders with the same Player A and campaign RNG.
