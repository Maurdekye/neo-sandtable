# Units — gaps and open questions

Format: `U-NNN` · area · what is missing or unclear · source checked · status.

- **U-001** · OA `Arrives` column order · RESOLVED: sheets and chart legends print `opstage/gt` (`land:4.45` says "Operation Stage/Game-Turn"; the legends read "1/39 Game-Turn", meaning OpStage 1 of GT 39). Schedules list GT then OpS. · resolved
- **U-002** · `it.1ccnn_div` sheet · the I(M) Tank Battalion row prints no ID code on that sheet; its canonical id and class come from the Libyan Tank Command sheet (`[[mention]]` on the 1 CCNN sheet). · resolved
- **U-003** · `it.75_27_gun` · the chart footnote says the gun's anti-armor counts as 0 until the first OpStage of GT 63; whether OpStage 1 of GT 63 is itself still 0 is ambiguous. Irrelevant to GT 1–6. Stored value 2 with a note; the engine rule is for rules-land. · open
- **U-004** · `it.c200` · the printed range reads ";40" (stray mark before the number); entered as 40. Not used in GT 1–6. · open, low
- **U-005** · `data/units/aircraft/cw.toml` · types needed through the Italian Campaign are entered (file header `partial = true`). · planned
- **U-006** · schedules · `schedules/*` cover all printed rows touching GT 1–20 (`partial = true` for the remaining campaign). · planned
- **U-007** · `it.tobruk_garrison` CD groups · only the 2nd, 4th and 5th groups print a number in parentheses (emplaced points that may barrage land targets); the 1st and 3rd print none, so no `barrage_land_max` is recorded for them. · open
- **U-008** · `it.unassigned_inf_recon` 22nd Bersaglieri Mitrg Coy · sheet indentation suggests assignment to the 10th Bersaglieri HQ, but the counter file shows no parent (and it arrives 1/10, before the HQ's 1/11); kept parentless. Arrives after GT 6. · open
- **U-009** · `it.tobruk_garrison.the_san_giorgio` · a fixed-ship counter with no printed class, rating or TOE on the sheet; its barrage and AA ratings are not on any chart read so far. · open
- **U-010** · `cw.70_inf_div` · the 1st Argyll & Sutherland arrival prints a lowercase "d" (taken as D); the 1st Durham Light Inf ID code prints a capital "M" (taken as m). Both are sibling-consistent. · noted
- **U-011** · `cw.selby_force` · the sheet is text only; the HQ counter's printed abbreviation is not on the sheet. "Selby" is taken from the counter file name `BR Selby - Matruh`. Class code a. · noted
- **U-012** · counters · stacking points are derived from echelon (`land:9.4`) and checked against the printed counters for all HQs of the set-up and a sample of other units; exceptions found: Aresca and Trivioli HQs (3). The counters' E/G flag letters are only reliable on the PNG renders, not the SVG ones (the SVG carries template letters), so the OA sheet's printed E superscript is the source for `engineer_hq`. · resolved for the set-up
- **U-013** · Italian sheets not in the Graziani set-up (Battle Groups, Littorio, German-support) beyond GT20 remain unentered. Sirte, Sabratha and Ariete are entered. · planned
- **U-014** · HQ `E` superscripts on OA sheets (7th Armoured Div HQ, 4th Indian, 70th, NZ, 6th Aus, 1st/2nd Libyan, Tobruk/Bardia Gaf HQs) are stored as `engineer_hq = true` per `land:23.14` (engineering capability); to be cross-checked with the counters' "E" marker. · in progress
- **U-015** · `cw.70_inf_div.1st_argyll_and_sutherland` · the OA sheet row reads "1st Argyll & Sutherland / 1 A&S" but the counter file shows "7 A&S" (parent 16/70); sheet value kept, counter text noted. · open, low
- **U-016** ? `it.libyan_inf_div` / `it.parachute_inf_div` artillery slot ? second pass with a 5x local zoom resolves the echelon mark as III; artillery regiment kind retained. ? resolved
- **U-017** · `ge.5_le_pz_div` second infantry slot · the number before the symbol is a smudge (neither a clear 1 nor 0); `sp` omitted. · open
- **U-018** · `ge.ramcke_bde` · the five printed slots use the solid-filled infantry symbol, which the legend assigns to the heavy weapons parachute infantry battalion; entered as printed (five such slots). The brigade's historical makeup suggests a plain parachute battalion glyph was meant. · open
- **U-019** · formations · OA units carry no `tags` (bersaglieri, motorized, machinegun, cavalry, armored car, recon, light/heavy AA ...), so kinds that depend on them cannot yet be tested against counters; add per-unit tags when the engine needs the slot check. · planned

- **U-020** · `it.unassigned_armored` · OA chart leaves all class codes blank. I(L), II(L), V(M), XXI(M) counters also show no ID code. Equipment and TOE counts are entered; `class` is omitted for all nine rows. · open
- **U-021** · Sirte 43rd / Ariete 132nd artillery regiments · OA sheets each print 6 light AA points, while class `it.kk` permits only 3. Preserve the OA weapon counts; formation legality requires a ruling. · open
- **U-022** · January 1941 air arrivals · airlog:34.84 narrative example mentions 59 planes (including 2 Wellingtons and 1 Maryland), but the schedule chart totals 57 (1 Wellington, no Maryland). The schedule chart supplies the data; narrative example is illustrative. · see interpretation units-0003
- **U-023** ? Rommel ? CPA60 from land:31.0; GT20 OpStage2 from land:4.43b; original PNG counter confirms stacking 0. ? resolved
- **U-024** ? 18th Australian Brigade ? counter and land:19.31 confirm stacking 3 despite the X brigade mark; entered as `super_brigade`, matching land:9.4. ? resolved
