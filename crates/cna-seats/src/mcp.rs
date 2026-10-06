//! The MCP tool server that gives each seat its own scoped tools (`docs/architecture.md` §5).
//!
//! Transport: MCP "streamable HTTP", answered with plain JSON bodies (no SSE stream), on the
//! loopback interface only. Each seat gets its own endpoint, `http://127.0.0.1:<port>/mcp/<token>`,
//! with an unguessable per-seat token. The token selects the seat; a tool call carries no seat
//! argument at all, so a seat cannot name another seat. All three CLIs accept an HTTP MCP server
//! with the URL alone, so no header support is needed.
//!
//! The router ([`ToolRouter`]) is independent of the transport so it can be unit-tested directly.

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::game::{GameBackend, SeatId, SeatInfo, SubmitRequest, ToolError};
use crate::memory::{SeatMemory, WriteMode};

/// The game shared between the tool server and the run supervisor.
pub type SharedGame = Arc<Mutex<dyn GameBackend>>;

/// Protocol revisions this server speaks; the newest is offered when the client asks for another.
const SUPPORTED_PROTOCOLS: [&str; 3] = ["2025-06-18", "2025-03-26", "2024-11-05"];

/// Names of the seat tools, in the order they are listed.
pub const TOOL_NAMES: [&str; 9] = [
    "observe",
    "inspect",
    "describe_actions",
    "validate",
    "submit",
    "message_team",
    "read_messages",
    "notebook_read",
    "notebook_write",
];

/// One executed tool call, reported for budgeting and the transcript.
#[derive(Clone, Debug)]
pub struct ToolEvent {
    pub seat: SeatId,
    pub tool: String,
    pub args: Value,
    pub ok: bool,
    /// Present when the call was a successful, non-duplicate `submit`.
    pub submitted: Option<(String, String)>,
}

/// Per-seat counters the run supervisor reads for its budgets.
#[derive(Default)]
pub struct SeatCounters {
    pub calls: AtomicU64,
}

/// Dispatches tool calls for seats. Holds no transport state.
pub struct ToolRouter {
    game: SharedGame,
    memory: Arc<dyn SeatMemory>,
    counters: BTreeMap<SeatId, Arc<SeatCounters>>,
    /// Hard refusal threshold per seat (calls). `None` = unlimited.
    call_cap: Option<u64>,
    events: Mutex<Option<mpsc::UnboundedSender<ToolEvent>>>,
}

impl ToolRouter {
    pub fn new(game: SharedGame, memory: Arc<dyn SeatMemory>, seats: &[SeatInfo]) -> Self {
        Self {
            game,
            memory,
            counters: seats
                .iter()
                .map(|s| (s.id.clone(), Arc::default()))
                .collect(),
            call_cap: None,
            events: Mutex::new(None),
        }
    }

    pub fn with_call_cap(mut self, cap: Option<u64>) -> Self {
        self.call_cap = cap;
        self
    }

    /// Subscribe to executed calls (one subscriber; a new call replaces the previous one).
    pub fn events(&self) -> mpsc::UnboundedReceiver<ToolEvent> {
        let (tx, rx) = mpsc::unbounded_channel();
        *self.events.lock().expect("events lock") = Some(tx);
        rx
    }

    pub fn counters(&self, seat: &str) -> Option<Arc<SeatCounters>> {
        self.counters.get(seat).cloned()
    }

    /// The `tools/list` result.
    pub fn tool_list() -> Value {
        let obj = |props: Value, required: &[&str]| {
            json!({ "type": "object", "properties": props, "required": required,
                    "additionalProperties": false })
        };
        json!({ "tools": [
            { "name": "observe",
              "description": "Your current situation report (only what you may know) and your pending decisions. Call this first, and after every submission.",
              "inputSchema": obj(json!({}), &[]) },
            { "name": "inspect",
              "description": "Authorized detail about one thing you can see, by target id (for the number duel: `hand`, or `round:<n>` for a resolved round).",
              "inputSchema": obj(json!({ "target": { "type": "string" } }), &["target"]) },
            { "name": "describe_actions",
              "description": "The legal action space of one of your pending decisions, with the domain of each parameter.",
              "inputSchema": obj(json!({ "decision_id": { "type": "string" } }), &["decision_id"]) },
            { "name": "validate",
              "description": "Check a draft response to a pending decision without committing it.",
              "inputSchema": obj(json!({ "decision_id": { "type": "string" },
                                         "response": { "type": "object" } }),
                                 &["decision_id", "response"]) },
            { "name": "submit",
              "description": "Commit your response to a pending decision. Final. Safe to repeat with the same response (idempotent).",
              "inputSchema": obj(json!({ "decision_id": { "type": "string" },
                                         "response": { "type": "object" } }),
                                 &["decision_id", "response"]) },
            { "name": "message_team",
              "description": "Send a short message to the other seats on your own side. Nobody on the other side can read it.",
              "inputSchema": obj(json!({ "text": { "type": "string" } }), &["text"]) },
            { "name": "read_messages",
              "description": "Read team messages sent to you, optionally only those after message number `after`.",
              "inputSchema": obj(json!({ "after": { "type": "integer", "minimum": 0 } }), &[]) },
            { "name": "notebook_read",
              "description": "Read your durable notebook. It survives restarts and handovers; your own memory may not. Keep standing plans and lessons here.",
              "inputSchema": obj(json!({}), &[]) },
            { "name": "notebook_write",
              "description": "Write your durable notebook (mode `replace` or `append`; default append). Limited to 32 KiB.",
              "inputSchema": obj(json!({ "text": { "type": "string" },
                                         "mode": { "type": "string", "enum": ["replace", "append"] } }),
                                 &["text"]) },
        ]})
    }

    /// Run one tool for `seat`. `epoch` is the controller epoch of the endpoint the call arrived on.
    pub fn call(&self, seat: &str, epoch: u64, tool: &str, args: &Value) -> Result<Value, String> {
        if let Some(c) = self.counters.get(seat) {
            let n = c.calls.fetch_add(1, Ordering::SeqCst) + 1;
            if self.call_cap.is_some_and(|cap| n > cap) {
                return Err("this seat's tool-call budget for the run is exhausted".into());
            }
        }
        let mut submitted = None;
        let result = self.dispatch(seat, epoch, tool, args, &mut submitted);
        if let Some(tx) = self.events.lock().expect("events lock").as_ref() {
            let _ = tx.send(ToolEvent {
                seat: seat.to_string(),
                tool: tool.to_string(),
                args: args.clone(),
                ok: result.is_ok(),
                submitted,
            });
        }
        result
    }

    fn dispatch(
        &self,
        seat: &str,
        epoch: u64,
        tool: &str,
        args: &Value,
        submitted: &mut Option<(String, String)>,
    ) -> Result<Value, String> {
        let text = |key: &str| -> Result<&str, String> {
            args.get(key)
                .and_then(Value::as_str)
                .ok_or_else(|| format!("missing string argument `{key}`"))
        };
        let tool_err = |e: ToolError| e.to_string();
        match tool {
            "observe" => Ok(self.game.lock().expect("game").observe(seat)),
            "inspect" => {
                let target = text("target")?;
                self.game
                    .lock()
                    .expect("game")
                    .inspect(seat, target)
                    .map_err(tool_err)
            }
            "describe_actions" => {
                let id = text("decision_id")?;
                self.game
                    .lock()
                    .expect("game")
                    .describe_actions(seat, id)
                    .map_err(tool_err)
            }
            "validate" => {
                let id = text("decision_id")?;
                let response = args.get("response").ok_or("missing argument `response`")?;
                self.game
                    .lock()
                    .expect("game")
                    .validate(seat, id, response)
                    .map_err(tool_err)
            }
            "submit" => {
                let id = text("decision_id")?;
                let response = args.get("response").ok_or("missing argument `response`")?;
                let receipt = self
                    .game
                    .lock()
                    .expect("game")
                    .submit(
                        seat,
                        SubmitRequest {
                            decision_id: id.to_string(),
                            epoch,
                            response: response.clone(),
                        },
                    )
                    .map_err(tool_err)?;
                if !receipt.duplicate {
                    *submitted = Some((receipt.decision_id.clone(), receipt.summary.clone()));
                }
                Ok(receipt.result)
            }
            "message_team" => {
                let n = self.memory.message_team(seat, text("text")?)?;
                Ok(json!({ "delivered_to": n }))
            }
            "read_messages" => {
                let after = args.get("after").and_then(Value::as_u64).unwrap_or(0);
                Ok(json!({ "messages": self.memory.read_messages(seat, after) }))
            }
            "notebook_read" => Ok(json!({ "notebook": self.memory.notebook_read(seat) })),
            "notebook_write" => {
                let mode = match args.get("mode").and_then(Value::as_str) {
                    None | Some("append") => WriteMode::Append,
                    Some("replace") => WriteMode::Replace,
                    Some(other) => return Err(format!("unknown mode `{other}`")),
                };
                let bytes = self.memory.notebook_write(seat, mode, text("text")?)?;
                Ok(json!({ "notebook_bytes": bytes }))
            }
            other => Err(format!("unknown tool `{other}`")),
        }
    }
}

/// What a token resolves to.
#[derive(Clone, Debug)]
struct Binding {
    seat: SeatId,
    epoch: u64,
    instructions: String,
}

struct ServerState {
    router: Arc<ToolRouter>,
    bindings: BTreeMap<String, Binding>,
}

/// A running tool server.
pub struct McpServer {
    addr: SocketAddr,
    tokens: BTreeMap<SeatId, String>,
    task: JoinHandle<()>,
    router: Arc<ToolRouter>,
}

/// How to bind one seat to the server.
#[derive(Clone, Debug)]
pub struct SeatEndpoint {
    pub seat: SeatId,
    pub epoch: u64,
    /// Served as the MCP `instructions` in `initialize`.
    pub instructions: String,
}

impl McpServer {
    /// Bind `127.0.0.1:0` and start serving. Tokens are random per start.
    pub async fn start(
        router: Arc<ToolRouter>,
        endpoints: Vec<SeatEndpoint>,
    ) -> std::io::Result<Self> {
        let mut bindings = BTreeMap::new();
        let mut tokens = BTreeMap::new();
        for e in endpoints {
            let token = format!(
                "{}{}",
                uuid::Uuid::new_v4().simple(),
                uuid::Uuid::new_v4().simple()
            );
            tokens.insert(e.seat.clone(), token.clone());
            bindings.insert(
                token,
                Binding {
                    seat: e.seat,
                    epoch: e.epoch,
                    instructions: e.instructions,
                },
            );
        }
        let state = Arc::new(ServerState {
            router: router.clone(),
            bindings,
        });
        let app = Router::new()
            .route(
                "/mcp/{token}",
                post(handle_post).get(no_stream).delete(no_stream),
            )
            .with_state(state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        Ok(Self {
            addr,
            tokens,
            task,
            router,
        })
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    pub fn router(&self) -> &Arc<ToolRouter> {
        &self.router
    }

    /// The URL one seat's CLI connects to.
    pub fn url(&self, seat: &str) -> Option<String> {
        self.tokens
            .get(seat)
            .map(|t| format!("http://127.0.0.1:{}/mcp/{t}", self.addr.port()))
    }

    pub fn shutdown(&self) {
        self.task.abort();
    }
}

impl Drop for McpServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn no_stream() -> StatusCode {
    // We never open a server-initiated SSE stream and never keep a server session to delete.
    StatusCode::METHOD_NOT_ALLOWED
}

async fn handle_post(
    State(state): State<Arc<ServerState>>,
    Path(token): Path<String>,
    body: String,
) -> Response {
    let Some(binding) = state.bindings.get(&token).cloned() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Ok(message) = serde_json::from_str::<Value>(&body) else {
        return json_response(
            StatusCode::BAD_REQUEST,
            &rpc_error(Value::Null, -32700, "parse error"),
        );
    };
    // Tool calls take the game lock and may block briefly; keep them off the async workers.
    let reply =
        tokio::task::spawn_blocking(move || handle_message(&state.router, &binding, message))
            .await
            .unwrap_or(None);
    match reply {
        Some(v) => json_response(StatusCode::OK, &v),
        None => StatusCode::ACCEPTED.into_response(),
    }
}

fn json_response(status: StatusCode, v: &Value) -> Response {
    (
        status,
        [("content-type", "application/json")],
        serde_json::to_string(v).unwrap_or_default(),
    )
        .into_response()
}

fn rpc_error(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

/// Handle one JSON-RPC message (or batch). `None` = nothing to send back (notifications).
fn handle_message(router: &ToolRouter, binding: &Binding, message: Value) -> Option<Value> {
    if let Value::Array(items) = message {
        let replies: Vec<Value> = items
            .into_iter()
            .filter_map(|m| handle_single(router, binding, &m))
            .collect();
        return if replies.is_empty() {
            None
        } else {
            Some(Value::Array(replies))
        };
    }
    handle_single(router, binding, &message)
}

fn handle_single(router: &ToolRouter, binding: &Binding, message: &Value) -> Option<Value> {
    let id = message.get("id").cloned();
    let method = message.get("method").and_then(Value::as_str);
    let Some(method) = method else {
        // A response or garbage; we never send requests, so ignore.
        return id.map(|id| rpc_error(id, -32600, "invalid request"));
    };
    let params = message.get("params").cloned().unwrap_or(Value::Null);
    let id = id?; // notifications get no reply
    let ok = |result: Value| json!({ "jsonrpc": "2.0", "id": id, "result": result });
    Some(match method {
        "initialize" => {
            let asked = params
                .get("protocolVersion")
                .and_then(Value::as_str)
                .unwrap_or("");
            let version = SUPPORTED_PROTOCOLS
                .iter()
                .find(|v| **v == asked)
                .unwrap_or(&SUPPORTED_PROTOCOLS[0]);
            ok(json!({
                "protocolVersion": version,
                "capabilities": { "tools": { "listChanged": false } },
                "serverInfo": { "name": "cna-seats", "version": env!("CARGO_PKG_VERSION") },
                "instructions": binding.instructions,
            }))
        }
        "ping" => ok(json!({})),
        "tools/list" => ok(ToolRouter::tool_list()),
        "tools/call" => {
            let name = params.get("name").and_then(Value::as_str).unwrap_or("");
            let args = params
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| json!({}));
            let (text, is_error) = match router.call(&binding.seat, binding.epoch, name, &args) {
                Ok(v) => (serde_json::to_string_pretty(&v).unwrap_or_default(), false),
                Err(e) => (e, true),
            };
            ok(json!({ "content": [{ "type": "text", "text": text }], "isError": is_error }))
        }
        "resources/list" => ok(json!({ "resources": [] })),
        "resources/templates/list" => ok(json!({ "resourceTemplates": [] })),
        "prompts/list" => ok(json!({ "prompts": [] })),
        _ => rpc_error(id, -32601, "method not found"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::InMemorySeatMemory;
    use crate::toy::NumberDuel;

    fn router() -> (ToolRouter, SharedGame) {
        let game = NumberDuel::new(5);
        let seats = game.seats();
        let game: SharedGame = Arc::new(Mutex::new(game));
        let memory = Arc::new(InMemorySeatMemory::new(seats.clone()));
        (ToolRouter::new(game.clone(), memory, &seats), game)
    }

    #[test]
    fn lists_exactly_the_nine_seat_tools() {
        let list = ToolRouter::tool_list();
        let names: Vec<&str> = list["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, TOOL_NAMES);
    }

    #[test]
    fn a_seat_only_sees_its_own_state() {
        let (r, _) = router();
        let a = r
            .call(NumberDuel::SEAT_A, 1, "observe", &json!({}))
            .unwrap();
        let b = r
            .call(NumberDuel::SEAT_B, 1, "observe", &json!({}))
            .unwrap();
        assert_eq!(a["you"], NumberDuel::SEAT_A);
        assert_eq!(b["you"], NumberDuel::SEAT_B);
        assert_ne!(a["your_hand"], b["your_hand"]);
        // A seat cannot answer another seat's decision by naming it.
        let b_decision = b["pending_decisions"][0]["id"].clone();
        let err = r
            .call(
                NumberDuel::SEAT_A,
                1,
                "submit",
                &json!({ "decision_id": b_decision, "response": { "card": 1 } }),
            )
            .unwrap_err();
        assert!(err.contains("unknown decision"), "{err}");
    }

    #[test]
    fn submit_reports_events_and_counts_calls() {
        let (r, game) = router();
        let mut rx = r.events();
        let seat = NumberDuel::SEAT_A;
        let obs = r.call(seat, 1, "observe", &json!({})).unwrap();
        let id = obs["pending_decisions"][0]["id"].clone();
        let card = obs["your_hand"][0].clone();
        let args = json!({ "decision_id": id, "response": { "card": card } });
        r.call(seat, 1, "submit", &args).unwrap();
        r.call(seat, 1, "submit", &args).unwrap(); // idempotent replay
        let _ = rx.try_recv().unwrap(); // observe
        let first = rx.try_recv().unwrap();
        assert_eq!(first.submitted.as_ref().unwrap().0, id.as_str().unwrap());
        let replay = rx.try_recv().unwrap();
        assert!(replay.submitted.is_none());
        assert_eq!(r.counters(seat).unwrap().calls.load(Ordering::SeqCst), 3);
        assert_eq!(game.lock().unwrap().pending(seat).len(), 0);
    }

    #[test]
    fn call_cap_refuses_instead_of_invoking() {
        let (r, game) = router();
        let r = r.with_call_cap(Some(1));
        let seat = NumberDuel::SEAT_A;
        r.call(seat, 1, "observe", &json!({})).unwrap();
        let err = r.call(seat, 1, "observe", &json!({})).unwrap_err();
        assert!(err.contains("budget"));
        assert_eq!(game.lock().unwrap().game_seq(), 1);
    }

    #[test]
    fn jsonrpc_handshake_and_errors() {
        let (r, _) = router();
        let b = Binding {
            seat: NumberDuel::SEAT_A.into(),
            epoch: 1,
            instructions: "play".into(),
        };
        let init = handle_message(
            &r,
            &b,
            json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize",
                    "params": { "protocolVersion": "2025-03-26" } }),
        )
        .unwrap();
        assert_eq!(init["result"]["protocolVersion"], "2025-03-26");
        assert_eq!(init["result"]["instructions"], "play");
        assert!(
            handle_message(
                &r,
                &b,
                json!({ "jsonrpc": "2.0", "method": "notifications/initialized" })
            )
            .is_none()
        );
        let bad = handle_message(
            &r,
            &b,
            json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/call",
                    "params": { "name": "bash", "arguments": { "command": "ls" } } }),
        )
        .unwrap();
        assert_eq!(bad["result"]["isError"], true);
        let nomethod =
            handle_message(&r, &b, json!({ "jsonrpc": "2.0", "id": 3, "method": "x" })).unwrap();
        assert_eq!(nomethod["error"]["code"], -32601);
    }
}
