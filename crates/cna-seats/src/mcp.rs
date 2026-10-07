//! The MCP tool server that gives each seat its own scoped tools (`docs/architecture.md` §5).
//!
//! Transport: MCP "streamable HTTP", answered with plain JSON bodies (no SSE stream), on the
//! loopback interface only. Each seat gets its own endpoint, `http://127.0.0.1:<port>/mcp/<token>`,
//! with an unguessable per-seat token. The token selects the seat; a tool call carries no seat
//! argument at all, so a seat cannot name another seat. All three CLIs accept an HTTP MCP server
//! with the URL alone, so no header support is needed.
//!
//! The router ([`ToolRouter`]) is independent of the transport so it can be tested directly.

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use cna_core::ids::SeatId;
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::game::{GameBackend, SubmitRequest};
use crate::memory::{SeatMemory, WriteMode};

/// The game shared between the tool server and the run supervisor.
pub type SharedGame = Arc<dyn GameBackend>;

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
    /// Present when the call was a successful, non-duplicate `submit`: (decision id, summary).
    pub submitted: Option<(String, String)>,
}

/// Per-seat counters the run supervisor reads for its budgets.
#[derive(Default)]
pub struct SeatCounters {
    pub calls: AtomicU64,
}

/// A trusted supervisor can require durable quota charging before every game call.
/// Failure refuses the call; it never runs an action whose charge was not committed.
pub trait ToolBudget: Send + Sync {
    fn charge(&self, seat: SeatId) -> Result<(), String>;
    fn authorize(
        &self,
        _seat: SeatId,
        _epoch: u64,
        _tool: &str,
        _args: &Value,
    ) -> Result<(), String> {
        Ok(())
    }
}

/// Dispatches tool calls for seats. Holds no transport state.
pub struct ToolRouter {
    game: SharedGame,
    memory: Arc<dyn SeatMemory>,
    counters: BTreeMap<SeatId, Arc<SeatCounters>>,
    /// Hard refusal threshold per seat (calls). `None` = unlimited.
    call_cap: Option<u64>,
    budget: Option<Arc<dyn ToolBudget>>,
    events: Mutex<Option<mpsc::UnboundedSender<ToolEvent>>>,
}

/// A stable 64-bit FNV-1a hash, for idempotency keys (not security).
fn fnv(text: &str) -> u64 {
    text.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    })
}

impl ToolRouter {
    pub fn new(game: SharedGame, memory: Arc<dyn SeatMemory>, seats: &[SeatId]) -> Self {
        Self {
            game,
            memory,
            counters: seats.iter().map(|s| (*s, Arc::default())).collect(),
            call_cap: None,
            budget: None,
            events: Mutex::new(None),
        }
    }

    pub fn with_budget(mut self, budget: Arc<dyn ToolBudget>) -> Self {
        self.budget = Some(budget);
        self
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

    pub fn counters(&self, seat: SeatId) -> Option<Arc<SeatCounters>> {
        self.counters.get(&seat).cloned()
    }

    pub fn game(&self) -> &SharedGame {
        &self.game
    }

    /// The `tools/list` result.
    pub fn tool_list() -> Value {
        let obj = |props: Value, required: &[&str]| {
            json!({ "type": "object", "properties": props, "required": required,
                    "additionalProperties": false })
        };
        let action = json!({ "description": "The chosen action, shaped like the `action_schema` that describe_actions returned for this decision. null passes where the decision allows it." });
        json!({ "tools": [
            { "name": "observe",
              "description": "Your current situation report (only what you may know) and your pending decisions. Call this first, and again after every submission.",
              "inputSchema": obj(json!({}), &[]) },
            { "name": "inspect",
              "description": "Authorized detail about one thing you can see, by target id. The observation says which targets exist.",
              "inputSchema": obj(json!({ "target": { "type": "string" } }), &["target"]) },
            { "name": "describe_actions",
              "description": "The legal action space of one of your pending decisions: a JSON Schema for the answer, plus the decision's context.",
              "inputSchema": obj(json!({ "decision_id": { "type": "string" } }), &["decision_id"]) },
            { "name": "validate",
              "description": "Check a draft answer to a pending decision without committing it. Changes nothing and uses no dice.",
              "inputSchema": obj(json!({ "decision_id": { "type": "string" }, "action": action }),
                                 &["decision_id", "action"]) },
            { "name": "submit",
              "description": "Commit your answer to a pending decision. Final. Submitting the identical answer twice is harmless (it is applied once).",
              "inputSchema": obj(json!({ "decision_id": { "type": "string" },
                                         "action": action,
                                         "revision": { "type": "integer", "description": "Decision revision you saw; omit to use the current one." },
                                         "explanation": { "type": "string", "description": "Optional brief rationale for your own side and the operator only. Commentary, never executable; keep it short to save output tokens." } }),
                                 &["decision_id", "action"]) },
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
    pub async fn call(
        &self,
        seat: SeatId,
        epoch: u64,
        tool: &str,
        args: &Value,
    ) -> Result<Value, String> {
        if let Some(budget) = &self.budget {
            budget.charge(seat)?;
            budget.authorize(seat, epoch, tool, args)?;
        }
        if let Some(c) = self.counters.get(&seat) {
            let n = c.calls.fetch_add(1, Ordering::SeqCst) + 1;
            if self.call_cap.is_some_and(|cap| n > cap) {
                return Err("this seat's tool-call budget for the run is exhausted".into());
            }
        }
        let mut submitted = None;
        let result = self.dispatch(seat, epoch, tool, args, &mut submitted).await;
        if let Some(tx) = self.events.lock().expect("events lock").as_ref() {
            let _ = tx.send(ToolEvent {
                seat,
                tool: tool.to_string(),
                args: args.clone(),
                ok: result.is_ok(),
                submitted,
            });
        }
        result
    }

    async fn dispatch(
        &self,
        seat: SeatId,
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
        let action = || {
            args.get("action")
                .ok_or_else(|| "missing argument `action`".to_string())
        };
        match tool {
            "observe" => {
                let observation = self.game.observe(seat).await;
                let pending: Vec<Value> = self
                    .game
                    .pending(seat)
                    .await
                    .into_iter()
                    .map(|d| {
                        json!({
                            "decision_id": d.id, "kind": d.kind, "revision": d.revision,
                            "summary": d.summary, "rules": d.rules, "secrecy": d.secrecy,
                            "list_answer": match &d.space.schema {
                                cna_core::decision::ActionSchema::List { min, max, .. } => Some(json!({"min_items":min,"max_items":max})),
                                _ => None,
                            },
                        })
                    })
                    .collect();
                let mut out = json!({
                    "game_seq": self.game.game_seq().await,
                    "observation": observation,
                    "pending_decisions": pending,
                });
                if let Some(o) = self.game.outcome().await {
                    out["outcome"] = o;
                }
                Ok(out)
            }
            "inspect" => self
                .game
                .inspect(seat, text("target")?)
                .await
                .map_err(|e| e.to_string()),
            "describe_actions" => self
                .game
                .describe_actions(seat, text("decision_id")?)
                .await
                .map_err(|e| e.to_string()),
            "validate" => self
                .game
                .validate(seat, text("decision_id")?, action()?)
                .await
                .map_err(|e| e.to_string()),
            "submit" => {
                let id = text("decision_id")?;
                let action = action()?;
                let revision = args
                    .get("revision")
                    .and_then(Value::as_u64)
                    .map(|r| u32::try_from(r).map_err(|_| "revision out of range".to_string()))
                    .transpose()?;
                // Exact-revision retries are idempotent. A reopened revision or new controller
                // is a different command even when it chooses the same action.
                let key = match revision {
                    Some(revision) => format!(
                        "{id}|e{epoch}|r{revision}|{:016x}",
                        fnv(&action.to_string())
                    ),
                    None => format!("{id}|{:016x}", fnv(&action.to_string())),
                };
                let receipt = self
                    .game
                    .submit(
                        seat,
                        SubmitRequest {
                            decision_id: id.to_string(),
                            epoch,
                            revision,
                            idempotency_key: key,
                            action: action.clone(),
                            public_explanation: args
                                .get("explanation")
                                .and_then(Value::as_str)
                                .map(str::to_string),
                        },
                    )
                    .await
                    .map_err(|e| e.to_string())?;
                if !receipt.duplicate {
                    *submitted = Some((receipt.decision_id.clone(), receipt.summary.clone()));
                }
                Ok(receipt.result)
            }
            "message_team" => {
                let n = self.memory.message_team(seat, text("text")?).await?;
                Ok(json!({ "delivered_to": n }))
            }
            "read_messages" => {
                let after = args.get("after").and_then(Value::as_u64).unwrap_or(0);
                Ok(json!({ "messages": self.memory.read_messages(seat, after).await }))
            }
            "notebook_read" => Ok(json!({ "notebook": self.memory.notebook_read(seat).await })),
            "notebook_write" => {
                let mode = match args.get("mode").and_then(Value::as_str) {
                    None | Some("append") => WriteMode::Append,
                    Some("replace") => WriteMode::Replace,
                    Some(other) => return Err(format!("unknown mode `{other}`")),
                };
                let bytes = self
                    .memory
                    .notebook_write(seat, mode, text("text")?)
                    .await?;
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
            tokens.insert(e.seat, token.clone());
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
    pub fn url(&self, seat: SeatId) -> Option<String> {
        self.tokens
            .get(&seat)
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
    match handle_message(&state.router, &binding, message).await {
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
async fn handle_message(router: &ToolRouter, binding: &Binding, message: Value) -> Option<Value> {
    if let Value::Array(items) = message {
        let mut replies = Vec::new();
        for m in items {
            if let Some(r) = handle_single(router, binding, &m).await {
                replies.push(r);
            }
        }
        return if replies.is_empty() {
            None
        } else {
            Some(Value::Array(replies))
        };
    }
    handle_single(router, binding, &message).await
}

async fn handle_single(router: &ToolRouter, binding: &Binding, message: &Value) -> Option<Value> {
    let id = message.get("id").cloned();
    let Some(method) = message.get("method").and_then(Value::as_str) else {
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
            let (text, is_error) = match router.call(binding.seat, binding.epoch, name, &args).await
            {
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
    use crate::toy::{AXIS, COMMONWEALTH, NumberDuel};

    fn router() -> (ToolRouter, SharedGame) {
        let game: SharedGame = Arc::new(NumberDuel::game(5));
        let seats = vec![AXIS, COMMONWEALTH];
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

    #[tokio::test]
    async fn a_seat_only_sees_its_own_state() {
        let (r, _) = router();
        let a = r.call(AXIS, 1, "observe", &json!({})).await.unwrap();
        let b = r
            .call(COMMONWEALTH, 1, "observe", &json!({}))
            .await
            .unwrap();
        assert_eq!(a["observation"]["you"], "axis.commander");
        assert_eq!(b["observation"]["you"], "commonwealth.commander");
        assert_ne!(a["observation"]["your_hand"], b["observation"]["your_hand"]);
        // A seat cannot answer another seat's decision by naming it.
        let b_decision = b["pending_decisions"][0]["decision_id"].clone();
        let err = r
            .call(
                AXIS,
                1,
                "submit",
                &json!({ "decision_id": b_decision, "action": "1" }),
            )
            .await
            .unwrap_err();
        assert!(err.contains("unknown decision"), "{err}");
    }

    #[tokio::test]
    async fn submit_reports_events_and_counts_calls() {
        let (r, game) = router();
        let mut rx = r.events();
        let obs = r.call(AXIS, 1, "observe", &json!({})).await.unwrap();
        let id = obs["pending_decisions"][0]["decision_id"].clone();
        let card = obs["observation"]["your_hand"][0].to_string();
        let args = json!({ "decision_id": id, "action": card });
        r.call(AXIS, 1, "submit", &args).await.unwrap();
        r.call(AXIS, 1, "submit", &args).await.unwrap(); // idempotent replay
        let _ = rx.try_recv().unwrap(); // observe
        let first = rx.try_recv().unwrap();
        assert_eq!(first.submitted.as_ref().unwrap().0, id.as_str().unwrap());
        let replay = rx.try_recv().unwrap();
        assert!(replay.ok && replay.submitted.is_none());
        assert_eq!(r.counters(AXIS).unwrap().calls.load(Ordering::SeqCst), 3);
        assert_eq!(game.pending(AXIS).await.len(), 0);
    }

    #[tokio::test]
    async fn describe_actions_exposes_the_schema_and_validate_is_pure() {
        let (r, game) = router();
        let obs = r.call(AXIS, 1, "observe", &json!({})).await.unwrap();
        let id = obs["pending_decisions"][0]["decision_id"].clone();
        let d = r
            .call(AXIS, 1, "describe_actions", &json!({ "decision_id": id }))
            .await
            .unwrap();
        assert_eq!(d["action_schema"]["type"], "string");
        assert!(d["action_schema"]["enum"].as_array().unwrap().len() == 5);
        let seq = game.game_seq().await;
        let bad = r
            .call(
                AXIS,
                1,
                "validate",
                &json!({ "decision_id": id, "action": "99" }),
            )
            .await;
        assert!(bad.unwrap_err().contains("illegal"));
        assert_eq!(game.game_seq().await, seq);
    }

    #[tokio::test]
    async fn call_cap_refuses_instead_of_invoking() {
        let (r, game) = router();
        let r = r.with_call_cap(Some(1));
        r.call(AXIS, 1, "observe", &json!({})).await.unwrap();
        let err = r.call(AXIS, 1, "observe", &json!({})).await.unwrap_err();
        assert!(err.contains("budget"));
        let _ = game;
    }

    #[tokio::test]
    async fn jsonrpc_handshake_and_errors() {
        let (r, _) = router();
        let b = Binding {
            seat: AXIS,
            epoch: 1,
            instructions: "play".into(),
        };
        let init = handle_message(
            &r,
            &b,
            json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize",
                    "params": { "protocolVersion": "2025-03-26" } }),
        )
        .await
        .unwrap();
        assert_eq!(init["result"]["protocolVersion"], "2025-03-26");
        assert_eq!(init["result"]["instructions"], "play");
        assert!(
            handle_message(
                &r,
                &b,
                json!({ "jsonrpc": "2.0", "method": "notifications/initialized" })
            )
            .await
            .is_none()
        );
        let bad = handle_message(
            &r,
            &b,
            json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/call",
                    "params": { "name": "bash", "arguments": { "command": "ls" } } }),
        )
        .await
        .unwrap();
        assert_eq!(bad["result"]["isError"], true);
        let nomethod = handle_message(&r, &b, json!({ "jsonrpc": "2.0", "id": 3, "method": "x" }))
            .await
            .unwrap();
        assert_eq!(nomethod["error"]["code"], -32601);
    }
    #[tokio::test]
    async fn identical_actions_at_reopened_revisions_are_distinct_but_retries_are_idempotent() {
        use crate::game::{GameBackend, SubmitReceipt, ToolError};
        // The serialized writer rejects a key reused for a different command. This backend
        // isolates that contract so reopened ids need no particular game's setup sequence.
        type RecordedCommand = (u64, Option<u32>, Value);
        #[derive(Default)]
        struct Reopened(Mutex<BTreeMap<String, RecordedCommand>>);
        #[async_trait::async_trait]
        impl GameBackend for Reopened {
            async fn seats(&self) -> Vec<SeatId> {
                vec![AXIS]
            }
            async fn game_seq(&self) -> u64 {
                0
            }
            async fn pending(&self, _: SeatId) -> Vec<cna_core::decision::DecisionRequest> {
                vec![]
            }
            async fn observe(&self, _: SeatId) -> Value {
                json!({})
            }
            async fn inspect(&self, _: SeatId, _: &str) -> Result<Value, ToolError> {
                Ok(json!({}))
            }
            async fn describe_actions(&self, _: SeatId, _: &str) -> Result<Value, ToolError> {
                Ok(json!({}))
            }
            async fn validate(&self, _: SeatId, _: &str, _: &Value) -> Result<Value, ToolError> {
                Ok(json!({}))
            }
            async fn epoch(&self, _: SeatId) -> u64 {
                1
            }
            async fn outcome(&self) -> Option<Value> {
                None
            }
            async fn submit(
                &self,
                _: SeatId,
                r: SubmitRequest,
            ) -> Result<SubmitReceipt, ToolError> {
                let mut commands = self.0.lock().unwrap();
                let command = (r.epoch, r.revision, r.action);
                let duplicate = if let Some(old) = commands.get(&r.idempotency_key) {
                    if old != &command {
                        return Err(ToolError::Other("idempotency conflict".into()));
                    }
                    true
                } else {
                    commands.insert(r.idempotency_key, command);
                    false
                };
                Ok(SubmitReceipt {
                    decision_id: r.decision_id,
                    duplicate,
                    summary: "accepted".into(),
                    result: json!({"duplicate":duplicate}),
                })
            }
        }
        let game = Arc::new(Reopened::default());
        let memory = Arc::new(InMemorySeatMemory::new(vec![AXIS]));
        let router = ToolRouter::new(game.clone(), memory, &[AXIS]);
        for (epoch, revision, duplicate) in [
            (1, 1, false),
            (1, 1, true),
            (1, 2, false),
            (1, 2, true),
            (2, 2, false),
        ] {
            let result = router
                .call(
                    AXIS,
                    epoch,
                    "submit",
                    &json!({"decision_id":"reopened","revision":revision,"action":null}),
                )
                .await
                .unwrap();
            assert_eq!(result["duplicate"], duplicate);
        }
        assert_eq!(game.0.lock().unwrap().len(), 3);
    }
}
