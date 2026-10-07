//! Offline exposure counts and real multi-order acceptance; never invokes a paid CLI.
mod support;
use async_trait::async_trait;
use cna_core::{
    decision::{ActionSchema, DecisionRequest},
    ids::{Role, SeatId, Side},
};
use cna_play::{
    Demo,
    config::{GameKind, LaunchConfig},
};
use cna_seats::{
    automatic,
    driver::{CliKind, DriverError, SeatDriver, SessionInfo, TurnOutcome, Usage},
    mcp::ToolRouter,
    run::PromptBuilder,
};
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::PathBuf, sync::Arc, time::Duration};

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}
fn first(schema: &ActionSchema) -> Value {
    match schema {
        ActionSchema::Choice { options } => {
            json!(options.first().expect("empty non-pass choice").id)
        }
        ActionSchema::Unit { among } => json!(among.first().expect("empty non-pass unit domain")),
        ActionSchema::Hex { among: Some(hexes) } => {
            json!(hexes.first().expect("empty non-pass hex domain"))
        }
        ActionSchema::Integer { min, max } => json!(if *min == 0 && *max > 0 { 1 } else { *min }),
        ActionSchema::Bool => json!(false),
        ActionSchema::Path { .. } => json!([]),
        ActionSchema::Record { fields } => Value::Object(
            fields
                .iter()
                .filter(|f| !f.optional)
                .map(|f| (f.name.clone(), first(&f.schema)))
                .collect(),
        ),
        ActionSchema::List { item, min, .. } => {
            Value::Array((0..*min).map(|_| first(item)).collect())
        }
        _ => panic!("fixture needs an enumerable action"),
    }
}
struct Fake {
    handle: cna_server::actor::CampaignHandle,
    router: Arc<ToolRouter>,
    seat: SeatId,
    epoch: u64,
    turns: u64,
    answer_sizes: Vec<u64>,
    id: Option<String>,
    moves: Vec<Value>,
    batch: usize,
    choose_moves: bool,
    destinations: BTreeMap<String, String>,
}
impl Fake {
    async fn call(&self, tool: &str, args: Value) -> Result<Value, DriverError> {
        self.router
            .call(self.seat, self.epoch, tool, &args)
            .await
            .map_err(DriverError::Cli)
    }
    // Select from this seat's current legal unit domain. Reachability and
    // restrictions come from inspect; never infer supply from a unit's name.
    async fn select_moves(
        &mut self,
        request: &DecisionRequest,
        observation: &Value,
    ) -> Result<(), DriverError> {
        let ActionSchema::List { item, max, .. } = &request.space.schema else {
            return Err(DriverError::Cli("movement did not offer a list".into()));
        };
        let ActionSchema::Record { fields } = item.as_ref() else {
            return Err(DriverError::Cli("movement items are not records".into()));
        };
        let Some(ActionSchema::Unit { among }) =
            fields.iter().find(|f| f.name == "unit").map(|f| &f.schema)
        else {
            return Err(DriverError::Cli(
                "movement has no disclosed unit domain".into(),
            ));
        };
        assert!(*max >= 2, "batch proof needs two offered units");
        let mut represented = std::collections::BTreeSet::new();
        // Inspect supplied candidates first to keep the inert planning bounded.
        // This is only a ranking: inspect and validate decide actual legality.
        let mut candidates: Vec<_> = among.iter().collect();
        candidates.sort_by_key(|unit| {
            let supply = &observation["logistics"]["unit_supply"][unit.as_str()];
            (
                std::cmp::Reverse(supply["tank_fuel"].as_i64().unwrap_or(0) > 0),
                std::cmp::Reverse(supply["activity_water"].as_i64().unwrap_or(0) > 0),
                unit.as_str(),
            )
        });
        for unit in candidates {
            let inspected = self.call("inspect", json!({"target":unit})).await?;
            let restrictions = inspected["movement_restrictions"]
                .as_array()
                .ok_or_else(|| DriverError::Cli("inspect omitted movement restrictions".into()))?;
            if restrictions.is_empty()
                || restrictions.iter().any(|r| {
                    r["restrictions"]["may_move"] != true
                        || r["unit"].as_str().is_none_or(|id| represented.contains(id))
                })
            {
                continue;
            }
            let origin = inspected["unit"]["hex"].as_str();
            let Some(path) = inspected["reachable"].as_array().and_then(|paths| {
                paths
                    .iter()
                    .filter(|p| {
                        p["path"].as_array().is_some_and(|p| p.len() == 1)
                            && p["hex"].as_str() != origin
                    })
                    .min_by_key(|p| {
                        (
                            p["cp_quarters"].as_i64().unwrap_or(i64::MAX),
                            p["hex"].as_str().unwrap_or(""),
                        )
                    })
            }) else {
                continue;
            };
            let order = json!({"unit":unit,"path":path["path"]});
            let mut planned = self.moves.clone();
            planned.push(order.clone());
            // Validate the ordered combined plan before committing any move.
            if self
                .call(
                    "validate",
                    json!({"decision_id":request.id,"action":planned}),
                )
                .await
                .is_err()
            {
                continue;
            }
            for restriction in restrictions {
                let id = restriction["unit"].as_str().unwrap().to_owned();
                represented.insert(id.clone());
                self.destinations
                    .insert(id, path["hex"].as_str().unwrap().to_owned());
            }
            self.moves.push(order);
            if self.moves.len() == 2 {
                return Ok(());
            }
        }
        Err(DriverError::Cli(format!(
            "only {} independent disclosed legal moves found",
            self.moves.len()
        )))
    }
}
#[async_trait]
impl SeatDriver for Fake {
    fn kind(&self) -> CliKind {
        CliKind::Claude
    }
    async fn start(&mut self, resume: Option<&str>) -> Result<SessionInfo, DriverError> {
        self.id = Some(
            resume
                .map(str::to_string)
                .unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
        );
        Ok(SessionInfo {
            session_id: self.id.clone().unwrap(),
            model: Some("offline".into()),
            cli_version: Some("inert".into()),
            resumed: resume.is_some(),
        })
    }
    async fn run_turn(&mut self, prompt: &str, _: Duration) -> Result<TurnOutcome, DriverError> {
        let pending = self
            .handle
            .seat(self.seat)
            .pending
            .first()
            .cloned()
            .ok_or_else(|| DriverError::Cli("no own pending window".into()))?;
        let observation = self.call("observe", json!({})).await?;
        let described = self
            .call("describe_actions", json!({"decision_id":pending.id}))
            .await?;
        let request: DecisionRequest =
            serde_json::from_value(described["request"].clone()).unwrap();
        if let ActionSchema::List { min, max, .. } = &request.space.schema {
            assert!(prompt.contains("One answer can carry a list"));
            let window = observation["pending_decisions"]
                .as_array()
                .unwrap()
                .iter()
                .find(|p| p["decision_id"] == request.id.as_str())
                .unwrap();
            assert_eq!(
                window["list_answer"],
                json!({"min_items":min,"max_items":max})
            );
        }
        if self.choose_moves {
            assert_eq!(request.kind, "cna.movement.orders");
            self.select_moves(&request, &observation["observation"])
                .await?;
            self.choose_moves = false;
        }
        let action = if !self.moves.is_empty() {
            assert_eq!(request.kind, "cna.movement.orders");
            let count = self.batch.min(self.moves.len());
            let moves: Vec<_> = self.moves.drain(..count).collect();
            for order in &moves {
                self.call("inspect", json!({"target":order["unit"]}))
                    .await?;
            }
            let action = Value::Array(moves);
            self.call(
                "validate",
                json!({"decision_id":request.id,"action":action}),
            )
            .await?;
            action
        } else if request.space.pass.is_some() {
            Value::Null
        } else {
            first(&request.space.schema)
        };
        self.call(
            "submit",
            json!({"decision_id":request.id,"revision":request.revision,"action":action}),
        )
        .await
        .map_err(|error| DriverError::Cli(format!("{}: {error}", request.kind)))?;
        if let Some(items) = action.as_array() {
            self.answer_sizes.push(items.len() as u64);
        }
        // The actor reply may precede watch publication. Wait for the exact
        // answered revision to disappear before sampling another model window.
        tokio::time::timeout(Duration::from_secs(30), async {
            let mut windows = self.handle.watch_seat(self.seat);
            while windows
                .borrow_and_update()
                .pending
                .iter()
                .any(|p| p.id == request.id && p.revision == request.revision)
            {
                windows
                    .changed()
                    .await
                    .map_err(|e| DriverError::Cli(e.to_string()))?;
            }
            Ok::<_, DriverError>(())
        })
        .await
        .map_err(|_| DriverError::Timeout)??;
        self.turns += 1;
        Ok(TurnOutcome {
            ok: true,
            error: None,
            text: None,
            usage: Usage::default(),
            quota: vec![],
        })
    }
    fn session_id(&self) -> Option<String> {
        self.id.clone()
    }
    fn is_alive(&mut self) -> bool {
        self.id.is_some()
    }
    async fn stop(&mut self) {
        self.id = None;
    }
}

#[tokio::test]
#[ignore = "slow: complete first real OpStage exposure measurement"]
async fn first_opstage_model_visible_windows_before_and_after_forced_answers() {
    let root = tempfile::tempdir().unwrap();
    let repo = repo();
    let demo = Demo::with_config(
        root.path(),
        &repo.join("data"),
        &repo.join("web/dist"),
        LaunchConfig::resolve(GameKind::Cna, &["*=human".into()]).unwrap(),
    )
    .await
    .unwrap();
    let seats: Vec<_> = Side::ALL
        .into_iter()
        .flat_map(|side| {
            Role::ALL
                .into_iter()
                .map(move |role| SeatId::new(side, role))
        })
        .collect();
    let backend = Arc::new(demo.handle.clone());
    let router = Arc::new(ToolRouter::new(backend.clone(), backend, &seats));
    let mut drivers: Vec<_> = seats
        .iter()
        .map(|seat| Fake {
            handle: demo.handle.clone(),
            router: router.clone(),
            seat: *seat,
            epoch: demo.handle.seat(*seat).binding.controller_epoch,
            turns: 0,
            answer_sizes: vec![],
            id: None,
            moves: vec![],
            batch: 1,
            choose_moves: false,
            destinations: BTreeMap::new(),
        })
        .collect();
    let mut counts: BTreeMap<String, [u64; 3]> =
        seats.iter().map(|s| (s.to_string(), [0, 0, 0])).collect();
    let mut accepted = 0;
    demo.handle.pause(false).await.unwrap();
    let start = tokio::time::Instant::now();
    // Measured 70s locally including preparation; 120s bounds stalls.
    let result = tokio::time::timeout(Duration::from_secs(120), async {
        loop {
            let mut progress = false;
            for driver in &mut drivers {
                let Some(request) = demo.handle.seat(driver.seat).pending.first().cloned() else {
                    continue;
                };
                if request.clock.game_turn > 1 || request.clock.op_stage.is_some_and(|op| op > 1) {
                    return Ok::<_, String>(());
                }
                let stage = request.clock.op_stage == Some(1);
                let forced = automatic::forced_pass(&request.space);
                if stage {
                    let count = counts.get_mut(&driver.seat.to_string()).unwrap();
                    count[0] += 1;
                    count[if forced { 2 } else { 1 }] += 1;
                }
                if forced {
                    automatic::answer(
                        &demo.handle,
                        driver.seat,
                        driver.epoch,
                        &request,
                        &demo.sink,
                    )
                    .await
                    .map_err(|e| e.to_string())?;
                } else {
                    if driver.id.is_none() {
                        driver.start(None).await.map_err(|e| e.to_string())?;
                    }
                    let prompt = demo
                        .prompts
                        .window_turn(driver.seat, std::slice::from_ref(&request));
                    driver
                        .run_turn(&prompt, Duration::from_secs(5))
                        .await
                        .map_err(|e| e.to_string())?;
                }
                accepted += 1;
                assert!(accepted < 5000, "inert trace safety cap");
                progress = true;
            }
            if !progress {
                match demo.handle.status() {
                    cna_server::CampaignStatus::Finished { .. } => return Ok(()),
                    cna_server::CampaignStatus::Running => {
                        tokio::time::sleep(Duration::from_millis(5)).await;
                    }
                    other => return Err(format!("trace has no pending window: {other:?}")),
                };
            }
        }
    })
    .await
    .map_err(|_| "first OpStage trace timed out".into())
    .and_then(|r| r);
    if matches!(demo.handle.status(), cna_server::CampaignStatus::Running) {
        demo.handle.pause(true).await.unwrap();
    }
    for driver in &mut drivers {
        driver.stop().await;
    }
    let cleanup = demo.shutdown().await;
    cna_play::combine_results(result, cleanup).unwrap();
    let total_before: u64 = counts.values().map(|c| c[0]).sum();
    let total_after: u64 = counts.values().map(|c| c[1]).sum();
    assert!(total_before > total_after && total_after > 0);
    for c in counts.values() {
        assert_eq!(c[0], c[1] + c[2]);
    }
    let report = json!({"measurement":"offline counterfactual model windows from identical first-OpStage fake-controller answers; not paid usage",
        "scenario":"graziani","profile":"cna-2021-dev","game_turn":1,"op_stage":1,"setup_excluded":true,
        "policy":"pass when offered; otherwise first enumerated choice with positive integer minima; no scripted supply baseline",
        "columns":["before","after","automatic"],"seats":counts,"total_before":total_before,"total_after":total_after,"elapsed_seconds":start.elapsed().as_secs_f64()});
    std::fs::write(
        repo.join("../seat-cost-opstage.json"),
        serde_json::to_string_pretty(&report).unwrap(),
    )
    .unwrap();
    println!("{}", serde_json::to_string_pretty(&report).unwrap());
}

async fn move_batch(batch: usize) -> (u64, Vec<u64>, BTreeMap<String, Value>) {
    let root = tempfile::tempdir().unwrap();
    let repo = repo();
    let mut config = LaunchConfig::resolve(
        GameKind::Cna,
        &[
            "axis.front_line=claude:haiku".into(),
            // Feed and water the actual movers before asking the inert driver to move.
            "axis.logistics=scripted:legal_random".into(),
            "*=scripted:pass_when_possible".into(),
        ],
    )
    .unwrap();
    config.max_turns = if batch == 1 { 2 } else { 1 };
    config.tool_calls = 32;
    let demo = support::movement_demo(root.path(), &repo, config).await;
    let mut fake = Fake {
        handle: demo.handle.clone(),
        // Inert planner may inspect the entire disclosed unit domain; this
        // fixture is counting answered windows, not using the paid CLI cap.
        router: Arc::new(
            ToolRouter::new(
                Arc::new(demo.handle.clone()),
                Arc::new(demo.handle.clone()),
                &[demo.seat],
            )
            .with_call_cap(Some(512)),
        ),
        seat: demo.seat,
        epoch: demo.epoch,
        turns: 0,
        answer_sizes: vec![],
        id: None,
        batch,
        moves: vec![],
        choose_moves: true,
        destinations: BTreeMap::new(),
    };
    let result = demo.play(&mut fake, if batch == 1 { 2 } else { 1 }).await;
    let sizes = fake.answer_sizes.clone();
    // Inert direct router calls don't emit a CLI stream, so verify the persisted
    // state and driver count rather than pretending a paid transcript exists.
    let view = demo
        .handle
        .projection(cna_core::visibility::Perspective::Seat(demo.seat));
    let turns = fake.turns;
    let cleanup = demo.shutdown().await;
    cna_play::combine_results(result, cleanup).unwrap();
    let mut final_units = BTreeMap::new();
    for (id, destination) in &fake.destinations {
        let unit = &view.view.units[id];
        assert_eq!(unit.hex.as_deref(), Some(destination.as_str()));
        final_units.insert(id.clone(), serde_json::to_value(unit).unwrap());
    }
    assert!(!final_units.is_empty());
    assert!(fake.moves.is_empty());
    (turns, sizes, final_units)
}
#[tokio::test]
async fn one_list_answer_moves_two_real_units() {
    let (turns, sizes, _) = move_batch(2).await;
    assert_eq!((turns, sizes), (1, vec![2]));
}
#[tokio::test]
#[ignore = "slow: two separate model windows; the batched movement proof remains default"]
async fn two_single_item_answers_move_the_same_real_units() {
    let (turns, sizes, _) = move_batch(1).await;
    assert_eq!((turns, sizes), (2, vec![1, 1]));
}
#[tokio::test]
#[ignore = "slow: compare both restored branches and identical mover state"]
async fn batched_and_single_answers_have_identical_real_unit_state() {
    let batch = move_batch(2).await;
    let singles = move_batch(1).await;
    assert_eq!(batch.2, singles.2);
    assert_eq!((batch.0, batch.1), (1, vec![2]));
    assert_eq!((singles.0, singles.1), (2, vec![1, 1]));
}
