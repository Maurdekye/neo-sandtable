# Area ownership

One owner per area at a time (see `CONTRIBUTING.md` §4). Owners are the agents or people
currently responsible; the lead agent updates this table when responsibility changes.

| Area | Paths | Owner |
|---|---|---|
| Project lead, docs, decisions, shared schemas (`data/*/README.md` envelopes, `docs/protocol.md`) | `docs/`, top-level files | neo-sandtable (lead agent) |
| Engine core | `crates/cna-core/` | neo-sandtable |
| Content loading and validation | `crates/cna-content/` | neo-sandtable |
| Server, runner, persistence | `crates/cna-server/` | neo-sandtable |
| AI seat drivers, MCP tool server, transcripts | `crates/cna-seats/` | seats |
| Map data and map tooling | `data/map/`, `tools/map/` | cartographer |
| Rule-case registry and tables: Land book | `data/rules/land/`, `data/tables/land/`, `tools/rules/` | rules-land |
| Rule-case registry and tables: Air & Logistics book | `data/rules/airlog/`, `data/tables/airlog/` | rules-airlog |
| Units, organization, equipment | `data/units/` | oob |
| Scenarios | `data/scenarios/` | oob |
| Live board | `web/` | board |
