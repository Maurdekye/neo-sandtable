# Units — gaps and open questions

Format: `U-NNN` · area · what is missing or unclear · source checked · status.

- **U-001** · OA `Arrives` column order · RESOLVED: sheets and chart legends print `opstage/gt` (`land:4.45` says "Operation Stage/Game-Turn"; the legends read "1/39 Game-Turn", meaning OpStage 1 of GT 39). Schedules list GT then OpS. · resolved
- **U-002** · `it.1ccnn_div` sheet · the I(M) Tank Battalion row prints no ID code on that sheet; its canonical id and class come from the Libyan Tank Command sheet (`[[mention]]` on the 1 CCNN sheet). · resolved
- **U-003** · `it.75_27_gun` · the chart footnote says the gun's anti-armor counts as 0 until the first OpStage of GT 63; whether OpStage 1 of GT 63 is itself still 0 is ambiguous. Irrelevant to GT 1–6. Stored value 2 with a note; the engine rule is for rules-land. · open
- **U-004** · `it.c200` · the printed range reads ";40" (stray mark before the number); entered as 40. Not used in GT 1–6. · open, low
- **U-005** · `data/units/aircraft/cw.toml` · only the types that occur in the Graziani set-up and schedules are entered so far (file header `partial = true`). · planned
- **U-006** · schedules · `schedules/*` are entered only for GT 1–6 (files carry `partial = true`); the whole war follows with the Italian Campaign. · planned
- **U-007** · `it.tobruk_garrison` CD groups · only the 2nd, 4th and 5th groups print a number in parentheses (emplaced points that may barrage land targets); the 1st and 3rd print none, so no `barrage_land_max` is recorded for them. · open
- **U-008** · `it.unassigned_inf_recon` 22nd Bersaglieri Mitrg Coy · sheet indentation suggests assignment to the 10th Bersaglieri HQ, but the counter file shows no parent (and it arrives 1/10, before the HQ's 1/11); kept parentless. Arrives after GT 6. · open
- **U-009** · `it.tobruk_garrison.the_san_giorgio` · a fixed-ship counter with no printed class, rating or TOE on the sheet; its barrage and AA ratings are not on any chart read so far. · open
- **U-010** · `cw.70_inf_div` · the 1st Argyll & Sutherland arrival prints a lowercase "d" (taken as D); the 1st Durham Light Inf ID code prints a capital "M" (taken as m). Both are sibling-consistent. · noted
- **U-011** · `cw.selby_force` · the sheet is text only; the HQ counter's printed abbreviation is not on the sheet. "Selby" is taken from the counter file name `BR Selby - Matruh`. Class code a. · noted
- **U-012** · counters · stacking points are derived from echelon (`land:9.4`) and checked against the printed counters for all HQs of the set-up and a sample of other units; exceptions found: Aresca and Trivioli HQs (3). The counters' E/G flag letters are only reliable on the PNG renders, not the SVG ones (the SVG carries template letters), so the OA sheet's printed E superscript is the source for `engineer_hq`. · resolved for the set-up
- **U-013** · Italian sheets not in the Graziani set-up (Sirte division, Battle Groups, Ariete, Littorio, German-support) are not yet entered; Sirte arrives GT 10. · planned
- **U-014** · HQ `E` superscripts on OA sheets (7th Armoured Div HQ, 4th Indian, 70th, NZ, 6th Aus, 1st/2nd Libyan, Tobruk/Bardia Gaf HQs) are stored as `engineer_hq = true` per `land:23.14` (engineering capability); to be cross-checked with the counters' "E" marker. · in progress
- **U-015** · `cw.70_inf_div.1st_argyll_and_sutherland` · the OA sheet row reads "1st Argyll & Sutherland / 1 A&S" but the counter file shows "7 A&S" (parent 16/70); sheet value kept, counter text noted. · open, low
