# Air & Logistics registry (airlog, sections 33-58)

Owner: rules-airlog. Schema: `../README.md`. One file per section, `NN-<slug>.toml`.

Conventions specific to this book:

- `[section]` is the `N.0` header; a `[[case]]` `N.0` exists only where the general rule carries
  rules text of its own (the tool counts both as one covered set).
- Fields `tables`, `depends_on`, `errata`, `interp` are omitted when empty.
- `errata = ["errata79:<case>"]` is used where the 2021 retype marks an inline correction,
  addition or clarification on the case (the retype integrates the 1979 addenda but does not
  say which printing a given marker came from). `errata79:54.17`, `41.5`, `42.53`, `46.4`, `30.59`
  are confirmed against the addenda list shipped with the sources.
- Chart tables are cited under the case number printed on the chart image (for example the
  Supply Dump Capacity Chart prints [54.12]; the text calls it 54.13). Cases that mention a chart
  list its table id in `tables`.
- Source typos handled in `tools/rules/check_registry.py` (`SOURCE_TEXT_FIXES`): `[53.1)`, `[56.l]`
  and `[31.53]` (read as 34.53).
- Sections 47 (Abstract Logistics) and 58 (Abstract Air) are registered with `graziani = "no"`:
  decision D2 excludes all official abstractions.
- Decision cases for the Logistics game that have no single rule case (stores allocation, water
  distribution) sit on the section's general-rule case (51.0, 52.0).
