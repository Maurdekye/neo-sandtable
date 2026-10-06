# cna-server

The first milestone is a library around `cna_core::engine::evaluate`. `Campaign<R>` exclusively
owns one SQLite connection and one campaign game. `step()` performs a single safe runner boundary:
advance automatic play, accept one scripted answer, or report an idle/paused seat. A caller must
serialize all access; the bounded actor, local HTTP API and WebSocket service are the next layer.

`Campaign::create` pins profile/content/engine versions and saves the initial full game.
`Campaign::recover` verifies the latest checkpoint and re-evaluates later commands, checking every
state/RNG and emitted-transition hash. Checkpoints are made every 32 accepted commands.

An accepted command, audience-tagged events, all 13 contiguous perspective streams, pending
windows, RNG, revision, lifecycle and optional checkpoint commit in one SQLite transaction. The
in-memory game changes only after commit. Rejections consume no campaign randomness. Submit
receipts contain no global revision or state hash. Idempotency keys are scoped per seat; exact
replays return duplicate receipts, while reuse for another command is refused.

Handover persists a new controller configuration and incremented epoch, retaining the game and
any accepted secret answer. A failed scripted controller pauses its seat without inventing an
order. `scripted::Candidates` is the ruleset adapter hook for paths and unenumerated hex domains.
The action generator has depth/node limits and a retry budget of 64; its own randomness never
uses the game's dice.

Transcript ingestion commits the seat's sequence and a separate alignment sequence for each
perspective allowed to see that seat. Perspective readers do not receive global event counts,
enemy transcripts, operator-only events, or enemy runtime metadata. Event readers are bounded
pages; live subscribers, resume/resync and slow-client isolation will be implemented in the actor
and WebSocket milestone.

Run `cargo test -p cna-server` for synthetic full-campaign, recovery, rollback injection, epoch,
idempotency, visibility, engine-stop and controller-failure checks. This is infrastructure with a
tiny synthetic ruleset, not a playable CNA scenario.
