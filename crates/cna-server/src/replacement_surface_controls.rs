//! Existing authenticated surfaces, ALL-EMPTY only; no neutral perspective or RP claim.
use super::*;
use crate::http::App;
use cna_protocol::ClientMessage;
use futures_util::{SinkExt, StreamExt};
use std::{error::Error as StdError, sync::Arc, time::Duration};
use tokio::{net::TcpListener, sync::oneshot, task::JoinHandle};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, tungstenite::Message};

type Checked<T> = Result<T, Box<dyn StdError + Send + Sync>>;
type Socket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

fn require(condition: bool, message: &str) -> Checked<()> {
    if condition {
        Ok(())
    } else {
        Err(std::io::Error::other(message.to_owned()).into())
    }
}

struct Api {
    app: App,
    handle: CampaignHandle,
    url: String,
    stop: oneshot::Sender<()>,
    server: JoinHandle<std::io::Result<()>>,
    client: reqwest::Client,
}

impl Api {
    async fn start(
        directory: &Path,
        path: &Path,
        campaign: Campaign<Cna>,
        profile: &str,
    ) -> Checked<Self> {
        // Finish all fallible listener/client construction before spawning the original writer.
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let port = listener.local_addr()?.port();
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .build()?;
        let handle = CampaignHandle::spawn_with_policy(
            campaign,
            path,
            movement_policy_with_profile(profile == cna_rules::PROFILE_FULL),
        )?;
        let app = App::new(
            directory.to_owned(),
            port,
            Arc::new(|_, _| Err(Error::Invalid("creation is outside this control".into()))),
        );
        app.register(handle.clone());
        let router = app.router(&directory.join("no-static-assets"));
        let (stop, wait) = oneshot::channel();
        let server = tokio::spawn(async move {
            axum::serve(listener, router)
                .with_graceful_shutdown(async {
                    let _ = wait.await;
                })
                .await
        });
        Ok(Self {
            app,
            handle,
            url: format!("http://127.0.0.1:{port}"),
            stop,
            server,
            client,
        })
    }

    fn token(&self, p: Perspective) -> String {
        let id = self.handle.header(p).meta.id;
        match p {
            Perspective::Operator => self.app.operator_token(),
            Perspective::Side(side) => self.app.side_token(&id, side).unwrap(),
            Perspective::Seat(seat) => self.app.seat_token(&id, seat).unwrap(),
        }
    }

    async fn get(&self, token: &str, path: &str) -> Checked<(u16, Vec<u8>)> {
        let response = self
            .client
            .get(format!("{}{path}", self.url))
            .bearer_auth(token)
            .send()
            .await?;
        let status = response.status().as_u16();
        Ok((status, response.bytes().await?.to_vec()))
    }

    // All assertions by the caller happen AFTER these original resources have joined.
    async fn finish(self) -> Checked<()> {
        let joined_writer = self.handle.shutdown().await;
        let _ = self.stop.send(());
        let joined_server = self.server.await;
        joined_writer?;
        joined_server??;
        Ok(())
    }
}

async fn next(socket: &mut Socket) -> Checked<Option<Vec<u8>>> {
    loop {
        match tokio::time::timeout(Duration::from_secs(5), socket.next()).await? {
            Some(Ok(Message::Text(text))) => return Ok(Some(text.as_bytes().to_vec())),
            Some(Ok(Message::Close(_))) | None => return Ok(None),
            Some(Ok(Message::Ping(_) | Message::Pong(_))) => {}
            Some(Ok(_)) => {
                return Err(std::io::Error::other("unexpected non-text server frame").into());
            }
            Some(Err(error)) => return Err(error.into()),
        }
    }
}

struct Live {
    perspective: Perspective,
    socket: Socket,
    seq: u64,
    transcripts: BTreeMap<SeatId, u64>,
    complete: Vec<Vec<u8>>,
}

impl Live {
    async fn open(api: &Api, p: Perspective) -> Checked<Self> {
        let projection = api.handle.projection(p);
        let hello = ServerMessage::Hello {
            protocol: cna_protocol::PROTOCOL_VERSION,
            campaign: projection.meta.clone(),
            perspective: p.to_string(),
        };
        let snapshot = ServerMessage::Snapshot {
            seq: projection.seq,
            view: projection.view,
        };
        let url = format!(
            "{}/api/campaigns/{}/stream?cap={}",
            api.url.replacen("http://", "ws://", 1),
            projection.meta.id,
            api.token(p)
        );
        let (mut socket, _) = tokio_tungstenite::connect_async(url).await?;
        socket
            .send(Message::Text(
                serde_json::to_string(&ClientMessage::Subscribe {
                    perspective: p.to_string(),
                    from_seq: None,
                })?
                .into(),
            ))
            .await?;
        let mut complete = vec![];
        for reference in [hello, snapshot] {
            let actual = next(&mut socket)
                .await?
                .ok_or_else(|| std::io::Error::other("short initial live WS"))?;
            require(
                actual == bytes(&reference),
                "entire authenticated live handshake differs",
            )?;
            complete.push(actual);
        }
        Ok(Self {
            perspective: p,
            socket,
            seq: projection.seq,
            transcripts: SeatId::all().map(|seat| (seat, 0)).collect(),
            complete,
        })
    }

    async fn through_commit(&mut self, api: &Api) -> Checked<()> {
        let target = api.handle.header(self.perspective).seq;
        let reader = api.handle.replay();
        let mut expected = vec![];
        while self.seq < target {
            let rows = reader.events(self.perspective, self.seq, target)?;
            require(!rows.is_empty(), "missing live reference page")?;
            for message in rows {
                let ServerMessage::Event { seq, .. } = &message else {
                    return Err(std::io::Error::other("non-event live reference").into());
                };
                require(*seq == self.seq + 1, "live perspective sequence gap")?;
                self.seq = *seq;
                expected.push(message);
            }
        }
        for seat in SeatId::all() {
            let cursor = self.transcripts.get_mut(&seat).unwrap();
            loop {
                let rows = reader.transcripts(self.perspective, seat, *cursor)?;
                let done = rows.len() < 512;
                for message in rows {
                    let ServerMessage::Transcript { tseq, .. } = &message else {
                        return Err(std::io::Error::other("non-transcript live reference").into());
                    };
                    require(*tseq == *cursor + 1, "live transcript sequence gap")?;
                    *cursor = *tseq;
                    expected.push(message);
                }
                if done {
                    break;
                }
            }
        }
        for reference in expected {
            let actual = next(&mut self.socket)
                .await?
                .ok_or_else(|| std::io::Error::other("short live commit publication"))?;
            require(
                actual == bytes(&reference),
                "entire live publication/order differs from this committed transition",
            )?;
            self.complete.push(actual);
        }
        Ok(())
    }

    async fn close(mut self) -> Checked<Vec<Vec<u8>>> {
        self.socket.close(None).await?;
        if let Some(text) = next(&mut self.socket).await? {
            self.complete.push(text);
            return Err(std::io::Error::other("unexpected trailing live publication").into());
        }
        Ok(self.complete)
    }
}

async fn ws(api: &Api, p: Perspective, replay: bool) -> Checked<Vec<Vec<u8>>> {
    let header = api.handle.header(p);
    let projection = api.handle.projection(p);
    let from = replay.then_some(0);
    let mut expected = vec![ServerMessage::Hello {
        protocol: cna_protocol::PROTOCOL_VERSION,
        campaign: header.meta.clone(),
        perspective: p.to_string(),
    }];
    if replay {
        let reader = api.handle.replay();
        let mut cursor = 0;
        while cursor < projection.seq {
            let rows = reader.events(p, cursor, projection.seq)?;
            require(
                !rows.is_empty(),
                "complete replay cannot silently lose a page",
            )?;
            for message in rows {
                let ServerMessage::Event { seq, .. } = &message else {
                    return Err(std::io::Error::other("non-event replay row").into());
                };
                require(*seq == cursor + 1, "perspective replay sequence gap")?;
                cursor = *seq;
                expected.push(message);
            }
        }
    } else {
        expected.push(ServerMessage::Snapshot {
            seq: projection.seq,
            view: projection.view,
        });
    }
    let reader = api.handle.replay();
    for seat in SeatId::all() {
        let mut cursor = 0;
        loop {
            let rows = reader.transcripts(p, seat, cursor)?;
            let done = rows.len() < 512;
            for message in rows {
                let ServerMessage::Transcript { tseq, .. } = &message else {
                    return Err(std::io::Error::other("non-transcript replay row").into());
                };
                require(*tseq == cursor + 1, "transcript replay sequence gap")?;
                cursor = *tseq;
                expected.push(message);
            }
            if done {
                break;
            }
        }
    }
    let url = format!(
        "{}/api/campaigns/{}/stream?cap={}",
        api.url.replacen("http://", "ws://", 1),
        header.meta.id,
        api.token(p)
    );
    let (mut socket, _) = tokio_tungstenite::connect_async(url).await?;
    let result = async {
        socket
            .send(Message::Text(
                serde_json::to_string(&ClientMessage::Subscribe {
                    perspective: p.to_string(),
                    from_seq: from,
                })?
                .into(),
            ))
            .await?;
        let mut actual = vec![];
        for reference in &expected {
            let text = next(&mut socket)
                .await?
                .ok_or_else(|| std::io::Error::other("short WebSocket history"))?;
            require(
                text == bytes(reference),
                "entire ordered WS message differs from committed reference",
            )?;
            actual.push(text);
        }
        Ok::<_, Box<dyn StdError + Send + Sync>>(actual)
    }
    .await;
    let close = socket.close(None).await;
    // Keep any unexpected remaining text instead of silently projecting it away.
    let mut tail = vec![];
    let drained = async {
        while let Some(text) = next(&mut socket).await? {
            tail.push(text);
        }
        Ok::<_, Box<dyn StdError + Send + Sync>>(())
    }
    .await;
    drop(socket);
    let mut actual = result?;
    close?;
    drained?;
    require(tail.is_empty(), "unexpected trailing WebSocket text")?;
    actual.extend(tail);
    Ok(actual)
}

#[derive(Debug, PartialEq)]
struct Surface {
    perspective: Perspective,
    // Complete status/body and ordered WS messages; no normalization or field omissions.
    http: Vec<(String, u16, Vec<u8>)>,
    snapshot_ws: Vec<Vec<u8>>,
    replay_ws: Vec<Vec<u8>>,
}

async fn capture(api: &Api, fixture: &Opened) -> Checked<Vec<Surface>> {
    let mut result = vec![];
    for p in Perspective::all() {
        let id = api.handle.header(p).meta.id;
        let token = api.token(p);
        let mut http = vec![];
        let mut paths = vec![
            format!("/api/campaigns/{id}?perspective={p}"),
            format!("/api/campaigns/{id}/seats?perspective={p}"),
        ];
        for seat in SeatId::all().filter(|seat| p.can_see(&Audience::Seat(*seat))) {
            paths.push(format!("/api/campaigns/{id}/seats/{seat}/observe"));
            for request in api.handle.seat(seat).pending {
                paths.push(format!(
                    "/api/campaigns/{id}/seats/{seat}/decisions/{}/actions",
                    request.id
                ));
            }
            // Actual unchanged own/opponent unit identities plus an unknown target;
            // a strict inspect refusal stays an exact status/body, never a stop/fallback.
            for side in cna_protocol::Side::ALL {
                let unit = fixture
                    .before
                    .state
                    .land
                    .units
                    .values()
                    .find(|unit| unit.side == side)
                    .ok_or_else(|| {
                        std::io::Error::other("unchanged content needs a real side unit")
                    })?;
                paths.push(format!(
                    "/api/campaigns/{id}/seats/{seat}/inspect/{}",
                    unit.id
                ));
            }
            paths.push(format!(
                "/api/campaigns/{id}/seats/{seat}/inspect/not-an-actual-unit"
            ));
            paths.push(format!(
                "/api/campaigns/{id}/transcripts?perspective={p}&seat={seat}&after=0"
            ));
        }
        for path in paths {
            let (status, body) = api.get(&token, &path).await?;
            if !path.contains("/inspect/") {
                require(status == 200, "authenticated existing surface rejected")?;
            }
            http.push((path, status, body));
        }
        result.push(Surface {
            perspective: p,
            http,
            snapshot_ws: ws(api, p, false).await?,
            replay_ws: ws(api, p, true).await?,
        });
    }
    Ok(result)
}

async fn exercise(fixture: &Opened, profile: &str) -> Checked<usize> {
    let directory = tempfile::tempdir()?;
    fixture.retain(directory.path());
    let path = directory.path().join("surfaces.sqlite");
    let (mut current, pins) = campaign(&path, profile, fixture.game.clone());
    let pending = current.pending();
    for request in &pending {
        bind(&mut current, request.seat);
    }
    // Responses are serialized through the real actor, with automatic advance paused.
    current.set_paused(true)?;
    let api = Api::start(directory.path(), &path, current, profile).await?;
    let result = async {
        let mut expected_game = fixture.game.clone();
        let mut expected_events = vec![];
        let mut surfaces = vec![capture(&api, fixture).await?];
        let mut live = vec![];
        for p in Perspective::all() { live.push(Live::open(&api, p).await?); }
        for request in &pending {
            let epoch = api.handle.seat(request.seat).binding.controller_epoch;
            let answer = scripted::answer(request, epoch, Mode::LegalRandom, 0, &NoCandidates)?;
            let canonical = canonical_pass(request, epoch);
            require(bytes(&answer) == bytes(&canonical), "complete response must equal independently built canonical pass")?;
            let expected = evaluate(&ruleset(profile)?, &fixture.content, &expected_game, &Command::Respond(canonical))?;
            let url = format!("{}/api/campaigns/{}/seats/{}/decisions/{}/submit",
                api.url, "replacement-actual-control", request.seat, request.id);
            let token = api.app.seat_token("replacement-actual-control", request.seat).unwrap();
            let submitted = api.client.post(&url).bearer_auth(&token).json(&answer).send().await?;
            require(submitted.status().as_u16() == 200, "canonical HTTP submission failed")?;
            let receipt: Value = submitted.json().await?;
            require(receipt["duplicate"] == false, "first submission must be new")?;
            let accepted_rows = checked_rows(&path)?;
            let duplicate = api.client.post(&url).bearer_auth(&token).json(&answer).send().await?;
            require(duplicate.status().as_u16() == 200, "HTTP duplicate failed")?;
            require(duplicate.json::<Value>().await?["duplicate"] == true, "HTTP duplicate must stay duplicate")?;
            require(checked_rows(&path)? == accepted_rows, "HTTP duplicate must preserve every SQLite row")?;
            for stream in &mut live { stream.through_commit(&api).await?; }
            expected_game = expected.game;
            expected_events.extend(expected.events);
            let after = capture(&api, fixture).await?;
            for (previous, next) in surfaces.last().unwrap().iter().zip(&after) {
                let opponent = match previous.perspective {
                    Perspective::Side(side) => side != request.seat.side,
                    Perspective::Seat(seat) => seat.side != request.seat.side,
                    Perspective::Operator => false,
                };
                if opponent {
                    require(previous == next, "complete opponent HTTP/pending/inspect/WS/replay output changed after hidden empty pass")?;
                }
            }
            surfaces.push(after);
            if !ruleset(profile)?.pending(&fixture.content, &expected_game.state).is_empty() {
                let waiting = evaluate(&ruleset(profile)?, &fixture.content, &expected_game, &Command::Advance)?;
                require(waiting.progress == Some(cna_core::engine::Progress::AwaitingDecisions)
                    && bytes(&waiting.game) == bytes(&expected_game) && waiting.events.is_empty(),
                    "unanswered fixed windows must block Advance without any state/RNG/event mutation")?;
            }
        }
        require(SeatId::all().all(|seat| api.handle.seat(seat).pending.is_empty()), "all six ordinary responses must consume actual windows")?;
        // Close every real socket before server shutdown; tail checking retains every
        // text frame, including unexpected opponent publication, rather than filtering it.
        let mut complete_live = vec![];
        for stream in live { complete_live.push(stream.close().await?); }
        require(complete_live.len() == 13, "all original authenticated viewers must close")?;
        // Independent COMPLETE Public stream includes the real DEV opening;
        // checkpoint0 stores the opened Game, so opening events are not fabricated DB rows.
        let public = |events: &[EngineEvent]| events.iter().filter(|event| event.audience == Audience::Public).cloned().collect::<Vec<_>>();
        let mut public_events = public(&fixture.events);
        public_events.extend(public(&expected_events));
        Ok::<_, Box<dyn StdError + Send + Sync>>((surfaces, expected_game, expected_events, public_events))
    }.await;
    // Both paths, including every fallible network/inspect/evaluation path, join first.
    let finished = api.finish().await;
    finished?;
    let (surfaces, expected_game, expected_events, public_events) = result?;
    let db = Connection::open(&path)?;
    let checkpoint: String =
        db.query_row("SELECT game FROM checkpoints WHERE revision=0", [], |row| {
            row.get(0)
        })?;
    require(
        bytes(&serde_json::from_str::<Game<Cna>>(&checkpoint)?) == bytes(&fixture.game),
        "actual checkpoint0 must retain the complete opened Game",
    )?;
    let mut query = db.prepare("SELECT payload FROM events ORDER BY event_id")?;
    let persisted = query
        .query_map([], |row| row.get::<_, String>(0))?
        .map(|row| Ok(serde_json::from_str::<EngineEvent>(&row?)?))
        .collect::<Checked<Vec<_>>>()?;
    require(
        bytes(&persisted) == bytes(&expected_events),
        "all committed EngineEvents must match canonical response transitions",
    )?;
    let mut actual_public: Vec<_> = fixture
        .events
        .iter()
        .filter(|event| event.audience == Audience::Public)
        .cloned()
        .collect();
    actual_public.extend(
        persisted
            .iter()
            .filter(|event| event.audience == Audience::Public)
            .cloned(),
    );
    require(
        bytes(&actual_public) == bytes(&public_events),
        "independent full ordered Public stream mismatch",
    )?;
    require(
        bytes(&actual_public)
            == bytes(
                &fixture
                    .events
                    .iter()
                    .filter(|event| event.audience == Audience::Public)
                    .cloned()
                    .collect::<Vec<_>>(),
            ),
        "six empty passes must not introduce Public activity",
    )?;
    let before = rows(&path);
    let recovery_content = CnaContent::load(
        &cna_content::repo_data_dir(),
        fixture.content.scenario_key(),
    )
    .map_err(Error::Invalid)?;
    let recovered = Campaign::recover(&path, ruleset(profile)?, recovery_content, &pins)?;
    require(
        bytes(recovered.game.as_ref()) == bytes(&expected_game),
        "complete recovered Game/RNG mismatch",
    )?;
    for surface in surfaces.last().unwrap() {
        let view = recovered.view(surface.perspective)?;
        let (_, _, body) = &surface.http[0];
        let http: Value = serde_json::from_slice(body)?;
        require(
            http["snapshot"]["view"] == serde_json::to_value(view)?,
            "recovered perspective projection differs",
        )?;
    }
    require(
        rows(&path) == before,
        "recovery must not mutate any SQLite row",
    )?;
    drop(recovered);
    // Recovered original writer and HTTP/WS replay follow the same existing interfaces.
    let recovery_content = CnaContent::load(
        &cna_content::repo_data_dir(),
        fixture.content.scenario_key(),
    )
    .map_err(Error::Invalid)?;
    let recovered = Campaign::recover(&path, ruleset(profile)?, recovery_content, &pins)?;
    let replay_api = Api::start(directory.path(), &path, recovered, profile).await?;
    let replayed = capture(&replay_api, fixture).await;
    let finished = replay_api.finish().await;
    finished?;
    require(
        replayed? == *surfaces.last().unwrap(),
        "all authenticated surfaces and complete WS replay differ after restart",
    )?;
    require(
        rows(&path) == before,
        "recovered writer and authenticated reads must preserve complete SQLite rows",
    )?;
    Ok(surfaces.len())
}

/// Cases: land:3.6, land:17.33, land:20.41, land:20.43, land:20.49
#[tokio::test]
async fn all_empty_exact_windows_preserve_authenticated_surfaces_and_public_history() {
    for kind in ["cna.replacements.training", "cna.replacements.allocation"] {
        let fixture = real_entered(kind);
        for profile in [cna_rules::PROFILE_DEV, cna_rules::PROFILE_FULL] {
            // Complete all13 outputs against the same committed history and recovery.
            // Transcripts keep real acceptance timestamps; two independent executions
            // cannot have equal wall-clock bytes and no timestamp is masked or forged.
            // Independently constructed canonical Respond is compared BEFORE submission;
            // whole Game/events/RNG are then compared against its direct evaluation.
            assert_eq!(exercise(&fixture, profile).await.unwrap(), 7); // before + six genuine responses
        }
    }
}
