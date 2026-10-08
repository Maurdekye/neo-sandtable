# air-0018 — Initial aircraft load commitment

- **Cases:** scen:59.32, scen:59.36, airlog:38.1, airlog:38.21, airlog:38.25, airlog:38.41, airlog:38.43, airlog:38.44, airlog:38.45, airlog:38.47
- **Status:** adopted
- **Profile version:** prospective Air preparation extension; no current profile change
- **Decided by:** neo-sandtable (lead), 2026-10-07
- **Owner review:** reviewed 2026-10-08 by the owner (batch review 4)

## Question

Initial aircraft have free fuel and arming, including those not refitted. The setup data and current aircraft flags do not identify a selected mode, gun load or bomb/mine/torpedo load. At what point does a fresh initial aircraft commit that unspecified load, and how may it change components before flying without receiving repeated free supplies?

The lead ruled this source question at 2026-10-07T16:22:59, and the owner adopted the proposed reading in batch review 4 on 2026-10-08. This document records the adopted source ruling without implementing it. Serialized provenance and the informational compatibility projection have separate lead approval in shape; the actual setup-close hook, preparation/flight debit cohorts, private answer windows and operational callers still require their own exact reviews.

## Evidence

scen:59.32 gives every scenario-initial aircraft fuel and arming without reducing scenario stocks. airlog:38.21 ties the ordinary refuel amount to an aircraft rating for a mission; different printed modes can have different costs. airlog:38.25 permits retaining fuel while grounded. airlog:38.41 separates flight from ammunition requirements. airlog:38.43 charges gun ammunition at squadron scope; airlog:38.44 and 38.47 charge carried ordnance at plane scope. airlog:38.45 consumes the bomb load on a mission even when bombing is unsuccessful or aborted. scen:59.36 prevents aircraft maintenance in the first Operations Stage; initial fuel and arming must therefore support the initial flight without requiring a maintenance step. These clauses do not explicitly identify the initial selected mode/payload or a preflight reconfiguration entitlement.

Existing setup import preserves only coarse fuelled/armed booleans and refit status. A checkpoint with these flags cannot prove whether initial load entitlement was previously used. Positive arrivals deliberately have no fuel, arming, refit or initial entitlement. These implementation facts establish the provenance problem, not extra game rights.

## Ruling

The adopted reading gives one complete free initial load for each setup-close-authenticated scenario-initial PlaneId: fuel for the explicitly chosen source mode, gun ammunition, and one source-permitted carried ordnance choice, which may be none. Arming is joint; there are no independent initial fuel, gun or ordnance credits. No amount, maximum load, capability or selected-mode default is invented. Later squadron gun pricing remains unchanged.

Commit this complete choice once at its first trusted known-load point: a preparation finish, or at the latest the first mission-declaration finish. The latter boundary is required by scen:59.36: first-stage maintenance is unavailable, while initial aircraft may still fly subject to their actual source eligibility and refit status. Recording the initial source-backed load is not a maintenance entitlement or an exception allowing ordinary first-stage maintenance. The published declaration schema currently supplies intentions only; extending a finish to establish this load requires separate exact caller and field approval.

The consumed one-time provenance survives checkpoint, later load use and map emptiness. Subsequent changes use ordinary source procedures only. An empty choice forfeits the single initial-load choice rather than preserving residual free entitlement; there is no second free configuration. Fuel or ammunition consumption during flight remains distinct from committing the initial choice and remains the separate source-defined operational procedure.

Only the real fresh setup-close lifecycle may authenticate the original scenario-initial PlaneIds and create initial provenance, on its reviewed disposable whole-State/Cx finish. A closed flag, GameTurn1, missing preparation map, generic inventory initialization or true compatibility flag cannot grant it. Absent serialized provenance means UnknownLegacy with no free credit; absence must never mean an unspent initial choice. Existing old saves resolve missing operational preparation as UnknownLegacy without free credit, inferred empty payload or automatic stock debit. New arrivals remain unprepared and have no initial entitlement.

This is an adopted source ruling without implementation in this document. The setup-close hook, costs/cohorts, answer windows and preparation/declaration/flight callers remain separately held. Approval of a serialized provenance shape or informational projection does not grant those procedural entitlements.

## Rationale

Commitment records one definite starting load while preserving the free initial state. It does not require enemy deployment facts to decide what was loaded, and it prevents repeated free reconfiguration before or after a flight. Fuel can remain prepared indefinitely; consuming a choice right is distinct from expending fuel or ammunition. The joint complete choice avoids an unspecified partially used arming credit buying gun and ordnance components at unrelated later times. Allowing commitment no later than the first mission declaration preserves initial flight under the first-stage maintenance restriction in scen:59.36.

## Recorded alternatives (rejected in batch review 4)

1. Spend initial choice rights at first actual deployment. This permits some preflight reconfiguration unless a separate reservation/locking rule is specified. The ruling would need exact repeated-plan, abort, unsuccessful scramble, emergency flight and checkpoint behavior before callers.
2. Commit fuel, gun and ordnance choices separately. This needs separate durable provenance for each entitlement, explicit treatment of a no-load choice and first-flight expiration of any unused rights. A single undifferentiated arming credit cannot support separate choices without deciding which component spends it. The owner rejected this alternative; no schema or caller adopts it.

## Affected behaviour and tests

There is no implemented preparation, initial-credit field, setup hook or flight caller. Future code needs a first-Operations-Stage initial flight with no maintenance under 59.36, complete initial choice at preparation or latest declaration finish, no independent arming credits or amount defaults, empty-choice forfeiture, fresh setup versus legacy initialized/uninitialized saves versus new-arrival proofs; preservation of IDs/counts/serials/refit; exactly once initial choice and checkpoint; no regenerated credit after load replacement or empty state; mode/gun/ordnance compatibility and no duplicated squadron debit; failed whole-State/Cx rollback including stock, flags, receipts and RNG; fixed private answer windows and every enemy mode/load/credit/stock pair. Pin each closed flag, GameTurn 1, missing preparation map, generic inventory initialization and true compatibility flag as a negative entitlement test. Only the real fresh setup-close lifecycle creates provenance; once spent, checkpoint recovery, an emptied map or load replacement cannot recreate it. Oob must review the actual setup-close hook, Airlog actual costs/cohorts, and rules-mgr the exact preparation/declaration/flight caller and private-window contracts before implementation.

## Separately approved informational compatibility projection

For a source-known load, the separately approved design projects AircraftState.armed as an informational projection of loaded guns OR actually present positive carried ordnance. It is never a firing/bombing entitlement and must be derived inside the single inventory update path. UnknownLegacy preserves its old coarse bit. Unprepared projects false; ready remains refitted, not composite flight readiness. Typed mode/load remains the operative truth. The lead approved this projection in shape on 2026-10-08. It must be derived in the single inventory update path, with its informational meaning documented. No legality, firing, bombing, mission-eligibility or flight-readiness check may read it. All actual readers must be enumerated before the projection lands; any such consumer must instead use typed load truth. The projection is not implemented by this interpretation commit.