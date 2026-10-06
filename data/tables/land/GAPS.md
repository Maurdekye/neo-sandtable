# Land chart gaps encountered by the typed bindings

- **land:8.89 — CPA 21–24.** The movement-distance image supplies columns for CPA at least 25, CPA 15–20, and CPA below 15. It supplies no result for 21–24. `OffMapMovement::stages` returns `None` for this interval; a procedure must report the unsupported input if it occurs. No interpolation is assumed.
- Existing transcription gaps for other Land charts remain recorded in [`../GAPS.md`](../GAPS.md), with interpretation proposals where a reading has been proposed.
- **land:29.6 — Game-Turn 111.** The weather chart ends its final winter interval at 110 and supplies no season for 111. The binding returns `None`; no seasonal extension is assumed.
