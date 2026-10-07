PRAGMA foreign_keys = ON;
CREATE TABLE IF NOT EXISTS campaign (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    meta TEXT NOT NULL, pins TEXT NOT NULL, revision INTEGER NOT NULL,
    rng TEXT NOT NULL, state_hash TEXT NOT NULL, status TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS commands (
    revision INTEGER PRIMARY KEY, seat TEXT NOT NULL, idempotency_key TEXT NOT NULL,
    command TEXT NOT NULL, state_hash TEXT NOT NULL, transition_hash TEXT NOT NULL,
    UNIQUE(seat, idempotency_key)
);
CREATE TABLE IF NOT EXISTS events (
    event_id INTEGER PRIMARY KEY, revision INTEGER NOT NULL REFERENCES commands(revision),
    payload TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS perspective_events (
    perspective TEXT NOT NULL, seq INTEGER NOT NULL, event_id INTEGER NOT NULL REFERENCES events(event_id),
    message TEXT NOT NULL, PRIMARY KEY(perspective, seq)
);
-- Per-perspective decision identities map directly to their committed stream position.
CREATE TABLE IF NOT EXISTS decision_opened (
    perspective TEXT NOT NULL, decision_id TEXT NOT NULL, seq INTEGER NOT NULL CHECK(seq > 0),
    PRIMARY KEY(perspective, decision_id),
    FOREIGN KEY(perspective, seq) REFERENCES perspective_events(perspective, seq)
);
CREATE TABLE IF NOT EXISTS checkpoints (
    revision INTEGER PRIMARY KEY, game TEXT NOT NULL, state_hash TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS decisions (
    id TEXT PRIMARY KEY, request TEXT NOT NULL, resolved_revision INTEGER REFERENCES commands(revision)
);
CREATE TABLE IF NOT EXISTS seats (seat TEXT PRIMARY KEY, binding TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS transcripts (
    seat TEXT NOT NULL, tseq INTEGER NOT NULL, at TEXT NOT NULL, entry TEXT NOT NULL,
    PRIMARY KEY(seat, tseq)
);
CREATE TABLE IF NOT EXISTS perspective_transcripts (
    perspective TEXT NOT NULL, seat TEXT NOT NULL, tseq INTEGER NOT NULL,
    game_seq INTEGER NOT NULL, PRIMARY KEY(perspective, seat, tseq),
    FOREIGN KEY(seat, tseq) REFERENCES transcripts(seat, tseq)
);
CREATE TABLE IF NOT EXISTS notebooks (seat TEXT NOT NULL, key TEXT NOT NULL, text TEXT NOT NULL, PRIMARY KEY(seat, key));
CREATE TABLE IF NOT EXISTS messages (id INTEGER PRIMARY KEY, side TEXT NOT NULL, seat TEXT NOT NULL, message TEXT NOT NULL);
