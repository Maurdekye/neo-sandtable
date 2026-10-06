//! Immutable replay pages are read on independent read-only connections, never the writer.
use crate::Error;
use cna_core::{
    ids::SeatId,
    visibility::{Audience, Perspective},
};
use cna_protocol::ServerMessage;
use rusqlite::{Connection, OpenFlags, params};
use std::path::{Path, PathBuf};

#[derive(Clone)]
pub struct ReplayReader {
    path: PathBuf,
}
impl ReplayReader {
    pub fn new(path: &Path) -> Self {
        Self {
            path: path.to_owned(),
        }
    }
    fn db(&self) -> Result<Connection, Error> {
        Ok(Connection::open_with_flags(
            &self.path,
            OpenFlags::SQLITE_OPEN_READ_ONLY,
        )?)
    }
    pub fn events(
        &self,
        perspective: Perspective,
        from: u64,
        through: u64,
    ) -> Result<Vec<ServerMessage>, Error> {
        let db = self.db()?;
        let mut stmt = db.prepare("SELECT message FROM perspective_events WHERE perspective=? AND seq>? AND seq<=? ORDER BY seq LIMIT 512")?;
        let rows = stmt.query_map(params![perspective.to_string(), from, through], |r| {
            r.get::<_, String>(0)
        })?;
        let mut result = Vec::new();
        for row in rows {
            result.push(serde_json::from_str(&row?)?);
        }
        Ok(result)
    }
    pub fn transcript_seq(&self, perspective: Perspective, seat: SeatId) -> Result<u64, Error> {
        if !perspective.can_see(&Audience::Seat(seat)) {
            return Ok(0);
        }
        Ok(self.db()?.query_row("SELECT COALESCE(MAX(tseq),0) FROM perspective_transcripts WHERE perspective=? AND seat=?", params![perspective.to_string(),seat.to_string()], |r| r.get(0))?)
    }
    pub fn transcripts(
        &self,
        perspective: Perspective,
        seat: SeatId,
        from: u64,
    ) -> Result<Vec<ServerMessage>, Error> {
        self.transcripts_through(perspective, seat, from, i64::MAX as u64)
    }
    pub fn transcripts_through(
        &self,
        perspective: Perspective,
        seat: SeatId,
        from: u64,
        through: u64,
    ) -> Result<Vec<ServerMessage>, Error> {
        if !perspective.can_see(&Audience::Seat(seat)) {
            return Ok(vec![]);
        }
        let db = self.db()?;
        let mut stmt = db.prepare("SELECT t.tseq,t.at,p.game_seq,t.entry FROM transcripts t JOIN perspective_transcripts p USING(seat,tseq) WHERE p.perspective=? AND t.seat=? AND t.tseq>? AND t.tseq<=? ORDER BY t.tseq LIMIT 512")?;
        let rows = stmt.query_map(
            params![perspective.to_string(), seat.to_string(), from, through],
            |r| {
                Ok((
                    r.get::<_, u64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, u64>(2)?,
                    r.get::<_, String>(3)?,
                ))
            },
        )?;
        let mut result = Vec::new();
        for row in rows {
            let (tseq, at, game_seq, entry) = row?;
            result.push(ServerMessage::Transcript {
                seat: seat.to_string(),
                tseq,
                at,
                game_seq,
                entry: serde_json::from_str(&entry)?,
            });
        }
        Ok(result)
    }
}
