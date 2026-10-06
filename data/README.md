# Game content data

Everything the engine knows about the game that is not procedure lives here as structured,
hand-reviewable data. `crates/cna-content` loads and validates it into hashed content packages.

| Folder | Contents |
|---|---|
| `map/` | Hex grid geometry, terrain per hex, hexside features, place names, facilities |
| `rules/` | The rule-case registry: every case of the baseline rulebooks and its disposition |
| `tables/` | Every chart and table (combat results, terrain effects, consumption rates, …) |
| `units/` | Unit types, equipment characteristics, organization (OA) charts, reinforcement schedules |
| `scenarios/` | Scenario setups, one folder per scenario (`graziani/` first) |

Each folder has its own `README.md` defining its exact schema. Its owner (see
`docs/ownership.md`) keeps that README accurate.

## Conventions

- **Formats:** TOML for structured records, CSV for large uniform tables (one header row,
  UTF-8, `\n` line endings). No spreadsheets, no binary files.
- **Citations:** every record carries a `src` field (or column) with one or more citations in the
  form defined in `CONTRIBUTING.md` §2, e.g. `src = ["land:8.37", "errata79:8.37"]`.
- **Units:** field names carry their unit when it is not obvious (`fuel_points`, `cpa`,
  `stacking_points`, `range_hexes`).
- **Ids:** lowercase `snake_case` or the game's own designations (`C4218` for hexes), stable
  forever once published. Display names are separate fields.
- **No invented values.** If the source does not give a value, leave it out and record the gap in
  the folder's `GAPS.md`, never a plausible guess.
- **No copied prose.** Notes are paraphrased in our own words (`CONTRIBUTING.md` §1).
