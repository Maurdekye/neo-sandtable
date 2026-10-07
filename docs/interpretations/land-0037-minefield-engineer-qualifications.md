# land-0037 - Minefield companion engineering qualifications

- **Cases:** land:23.13, land:23.14, land:23.15, land:23.21, land:26.24, land:26.25, land:6.3
- **Status:** proposed
- **Classification:** consequential
- **Profile version:** v1
- **Decided by:** neo-sandtable via rules-mgr, 2026-10-07
- **Owner review:** pending batch3
- **Lead approval:** approved as written by neo-sandtable via rules-mgr, 2026-10-07

## Question
Does the broad companion wording in23.21 permit every engineer company and engineering HQ to provide all minefield benefits, or do the specific restrictions in26.24 and26.25 govern each benefit?

## Evidence
Case23.21 describes a companion of any engineering unit or capable HQ. Case26.24 identifies battalions and Commonwealth capable HQs for entry into enemy fields; its friendly-field companion clause identifies battalions. Case26.25 identifies battalions and Commonwealth capable HQs for vehicle-loss protection. Case23.15 gives source-bound Scorpion battalions anti-mine engineering status only while at least six actual Scorpion points remain. Case23.13 restricts special road and railway units to their work. Adoptedland0015 already settles the numeric price conflict; this proposal does not reopen it.

## Ruling
Use26.24 and26.25 for accompanying-unit qualifications. A general engineer battalion grants the applicable companion benefits. A Commonwealth HQ with verified engineering capability grants the enemy-field entry and vehicle-loss protection named for it; do not extend its companion exemption to the friendly-field clause that lists battalions. An ordinary general engineer company does not grant battalion accompaniment or loss protection to other units. Anti-mine-only Scorpion battalions qualify for these anti-mine benefits only while the actual identified Scorpion TOE is at least six; a designation, reinforcement date or former composition never supplies that threshold. Road-only and railroad-only engineering status is not generalized into these benefits.

Keep the engineer actor's own entry eligibility separate from companion protection. In particular, the company actor's own engineer entry treatment does not make its companions battalion-protected. This file selects accompaniment/loss qualifications only; it does not infer a self-HQ benefit omitted by a specific case. Missing metadata stays unresolved, while explicit verified none and a failed actual Scorpion threshold are known failures for this action. Use the adopted0015 bound costs after determining the relevant qualification.

## Alternatives and rationale
The broad23.21 reading would let companies and capable HQs of any side protect companions. Applying the specific companion and vehicle-loss cases retains their deliberate grade and side requirements. Applying the enemy-HQ wording to friendly fields would add a benefit absent from that clause. Reading Scorpion as a permanent class would lose the explicit current-strength condition. The lead directs the specific-case reading, separate self eligibility and no generalization of special scopes; this draft makes those differences reviewable before any protection boolean is implemented.

## Affected behaviour and tests
Planned source-driven matrices cover general company versus battalion; Commonwealth versus other-side engineering HQ; enemy versus friendly CP qualification; vehicle protection separately from CP; company self treatment versus companions; Scorpion exactly5/6/7 actual points, absent weapon points and no automatic refit; restricted road/rail scopes; explicit verified none versus missing metadata. Pair tests compare enemy views, public counter-presence cost opportunities, hidden field type and private composition. Source qualities never change public opportunities or own/public answer acceptance. Costs remain the bound0015 values. Tests and runtime qualifiers are not yet implemented.