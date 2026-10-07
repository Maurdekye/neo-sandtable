# airlog-0015 ? Forced CPA and retained activity-water credit

- **Cases:** land:6.13, land:8.56, airlog:52.42, airlog:52.43, airlog:52.51
- **Status:** proposed
- **Profile version:** cna-2021-dev; cna-2021-full
- **Decided by:** rules-airlog, with neo-sandtable approval, 2026-10-07
- **Owner review:** pending batch 2

## Question
Involuntary CP can require activity water when the receiving unit lacks enough. Rejecting the action would prevent incoming fire. Later casualties and truck detachments can also change the unit's current composition after its first activity, so recomputing the original obligation could erase shortages or charge transferred trucks twice.

## Evidence
The activity rule charges vehicles and trucks on CPA use (52.42), while incoming fire can impose CP (6.13) and the dry restrictions constrain movement and offensive assault (52.51). The source does not define partial-payment bookkeeping or transfer of its credit under detachment (8.56).

## Ruling
At first forced CPA use, consume the smaller of the retained reserve and the original activity requirement. Record that requirement and the cumulative payment for the OpStage. The action proceeds despite a known shortage. Repeated forced CP consumes no further water. A later voluntary expenditure can settle the unpaid balance after rewatering; it cannot spend until enough reserve is available. Unfunded balances retain the ordinary dry restrictions. A new OpStage starts a new obligation. Unknown vehicle composition remains unsupported in full and privately unassessed in development.

Keep body and truck obligations separate. For partial payments, allocate body water first, then Light, Medium and Heavy truck water. The source does not prescribe this allocation; the convention makes bookkeeping deterministic. An explicit truck transfer moves the corresponding truck obligation and paid credit, taking paid credit first within each transferred type. It never transfers the body's payment. Retained reserve water is divided explicitly as a separate holding. Preserve the original body obligation through subsequent casualties.

## Rationale
Incoming fire cannot gain immunity through a known water shortage. Partial payments, recovery and later rewatering preserve the same balance. Transferred trucks carry their paid credit without making the receiving unit's own body paid. No additional water is consumed by transferring credit.

## Affected behaviour and tests
Tests cover partial forced payments, repeated forced use, rewatering, casualty changes, hot-weather rates, checkpoints, new stages, body-versus-truck credit and atomic transfer rejection.
