//! Synthetic instances of the inventoried production shapes, plus a legacy oracle.
//! No rules quantities, campaign RNG, content files or controller hooks are altered.
use super::*;
use cna_core::{
    clock::{Anchor, Clock},
    decision::{ActionSpace, ChoiceOption, FieldSchema, Secrecy, Trigger},
    ids::{Role, Side},
};
use serde_json::json;
use std::cell::Cell;

fn request(kind: &str, schema: ActionSchema, pass: bool) -> DecisionRequest {
    let mut space = ActionSpace::new(schema);
    if pass {
        space = space.with_pass("Declared fixture pass");
    }
    DecisionRequest {
        id: format!("fixture:{kind}").as_str().into(),
        seat: SeatId::new(Side::Axis, Role::Air),
        kind: kind.into(),
        revision: 3,
        clock: Clock::start(1, Anchor::new("fixture")),
        summary: "Synthetic schema instance".into(),
        rules: vec![],
        trigger: Trigger::Scheduled,
        secrecy: Secrecy::SecretSimultaneous,
        space,
    }
}

/// The pre-policy answer path, using the unchanged real Generator and sampler.
/// Keep this independent of ResourcePreview and the new unavailable-budget path.
fn legacy_answer(
    request: &DecisionRequest,
    epoch: u64,
    mode: Mode,
    attempt: u32,
    candidates: &dyn Candidates,
) -> Result<DecisionResponse, Error> {
    let seed: [u8; 32] = Sha256::digest(serde_json::to_vec(&(request, epoch, attempt))?).into();
    let mut generator = Generator {
        request,
        rng: ChaCha8Rng::from_seed(seed),
        nodes_left: 4096,
        candidates,
        attempt,
    };
    let unavailable = unavailable_domain(request, candidates);
    if unavailable && request.space.pass.is_none() {
        return Err(Error::Invalid(format!(
            "scripted action domain unavailable for {}: ruleset candidate hook required and no declared pass",
            request.kind
        )));
    }
    let pass = request.space.pass.is_some()
        && (unavailable || mode == Mode::PassWhenPossible || sample(&mut generator.rng, 5)? == 0);
    let action = if pass {
        Value::Null
    } else {
        generator.action(&request.space.schema, 0)?
    };
    Ok(DecisionResponse {
        decision_id: request.id.clone(),
        seat: request.seat,
        controller_epoch: epoch,
        decision_revision: request.revision,
        idempotency_key: format!(
            "scripted:{}:{epoch}:{}:{attempt}",
            request.id, request.revision
        ),
        action,
        public_explanation: None,
    })
}

fn integer(max: i64) -> ActionSchema {
    ActionSchema::Integer { min: 0, max }
}
fn choice(n: u32) -> ActionSchema {
    ActionSchema::Choice {
        options: (0..n)
            .map(|i| ChoiceOption {
                id: format!("fixture-{i}"),
                label: format!("Fixture {i}"),
                detail: None,
            })
            .collect(),
    }
}
fn field(name: &str, schema: ActionSchema, optional: bool) -> FieldSchema {
    FieldSchema {
        name: name.into(),
        doc: name.into(),
        schema,
        optional,
    }
}
fn record(fields: Vec<FieldSchema>) -> ActionSchema {
    ActionSchema::Record { fields }
}
fn list(item: ActionSchema, min: u32, max: u32) -> ActionSchema {
    ActionSchema::List {
        item: Box::new(item),
        min,
        max,
    }
}
fn unit() -> ActionSchema {
    ActionSchema::Unit {
        among: vec!["fixture-a".into(), "fixture-b".into()],
    }
}
fn hex(enumerated: bool) -> ActionSchema {
    ActionSchema::Hex {
        among: enumerated.then(|| vec!["C4218".into(), "C4219".into()]),
    }
}
fn amounts(optional: bool) -> ActionSchema {
    record(
        ["ammo", "fuel", "stores", "water"]
            .into_iter()
            .map(|name| field(name, integer(i64::from(i32::MAX)), optional))
            .collect(),
    )
}

// coastal.rs803/690: 1+8*n nodes, maximum8193, one item9.
fn coastal() -> ActionSchema {
    list(
        record(vec![
            field("from", choice(3), false),
            field("to", choice(3), false),
            field("cargo", amounts(false), false),
        ]),
        0,
        1024,
    )
}

// window.rs246: use later maintenance, retaining plane-count cohort maxima.
// The first-stage source-barred cohorts0 correction is deliberately not this fixture.
fn maintenance(planes: u32, sgsus: u32, dumps: u32) -> ActionSchema {
    let amount = record(vec![
        field("fuel", integer(i64::from(i32::MAX)), false),
        field("ammo", integer(0), false),
        field("stores", integer(i64::from(i32::MAX)), false),
        field("water", integer(i64::from(i32::MAX)), false),
    ]);
    let draw = record(vec![
        field(
            "source",
            record(vec![
                field("source", choice(1), false),
                field("id", choice(dumps), false),
            ]),
            false,
        ),
        field("amount", amount, false),
    ]);
    record(vec![
        field("dues", list(choice(sgsus), 0, sgsus), false),
        field(
            "cohorts",
            list(
                record(vec![
                    field("servicer", choice(sgsus), false),
                    field("assigned_squadron", choice(3), false),
                    field("priority", list(choice(planes), 0, planes), false),
                    field("method", choice(2), false),
                ]),
                0,
                planes,
            ),
            false,
        ),
        field(
            "draws",
            list(
                record(vec![
                    field("servicer", choice(sgsus), false),
                    field("draws", list(draw, 0, dumps), false),
                ]),
                0,
                sgsus,
            ),
            false,
        ),
    ])
}

/// Shape fixtures, not claims of captured historical requests or invented game data.
/// Cover each constructor family, including its empty/optional/nested branches.
fn production_shapes() -> Vec<(&'static str, ActionSchema, bool)> {
    let draws = list(
        record(vec![
            field("source", choice(3), false),
            field("amount", amounts(false), false),
        ]),
        0,
        8,
    );
    let allocation = record(vec![
        field("unit", unit(), false),
        field("count", integer(i64::from(i32::MAX)), false),
        field("draws", draws.clone(), false),
    ]);
    let packing = record(
        ["light", "medium", "heavy"]
            .into_iter()
            .map(|name| field(name, amounts(true), true))
            .collect(),
    );
    let opaque_orders = list(
        record(vec![
            field("unit", unit(), false),
            field("path", list(hex(false), 0, 50), false),
            field("with_stack", ActionSchema::Bool, true),
            field("close_assault", list(hex(false), 0, 0), true),
        ]),
        0,
        6,
    );
    let combat = list(
        record(vec![
            field("unit", unit(), false),
            field("target", hex(true), false),
            field("combat", integer(4095), false),
            field("draws", draws.clone(), false),
            field("flag", ActionSchema::Bool, true),
        ]),
        0,
        6,
    );
    vec![
        ("cna.initiative_declaration", choice(2), false),
        ("cna.setup.unit", choice(4), false),
        ("cna.setup.dump", choice(4), false),
        (
            "cna.setup.first_line_trucks",
            record(vec![
                field("unit", unit(), false),
                field("truck", integer(8), false),
            ]),
            false,
        ),
        ("cna.setup.truck_pool", packing.clone(), false),
        ("cna.setup.preload.choice", choice(3), false),
        ("cna.setup.preload.record", amounts(false), false),
        ("cna.setup.air", choice(4), false),
        (
            "cna.arrivals.batch",
            record(vec![
                field("place", choice(3), false),
                field("pool", choice(3), false),
                field("trucks", packing.clone(), false),
                field("substitute", choice(3), false),
                field("transport", list(choice(6), 0, 6), false),
                field(
                    "air",
                    record(vec![
                        field("quota", choice(2), false),
                        field("planes", amounts(false), false),
                    ]),
                    false,
                ),
            ]),
            false,
        ),
        ("cna.arrivals.empty", choice(0), true),
        (
            "cna.combat.position",
            list(
                record(vec![
                    field("unit", unit(), false),
                    field("position", choice(2), false),
                ]),
                0,
                6,
            ),
            true,
        ),
        ("cna.combat.force_assignment", combat.clone(), true),
        ("cna.combat.barrage_hexes", list(hex(true), 0, 6), true),
        ("cna.combat.barrage", combat.clone(), true),
        ("cna.combat.barrage_losses", combat.clone(), false),
        ("cna.combat.barrage_losses.empty", list(unit(), 0, 0), true),
        ("cna.combat.retreat", opaque_orders.clone(), true),
        ("cna.movement.cycle", ActionSchema::Bool, true),
        ("cna.movement.orders", opaque_orders.clone(), true),
        ("cna.movement.reaction", opaque_orders.clone(), true),
        ("cna.movement.continue", opaque_orders.clone(), false),
        ("cna.reserve.designate", list(unit(), 0, 6), true),
        ("cna.reserve.release", list(unit(), 0, 6), true),
        ("cna.breakdown.unit", allocation.clone(), false),
        (
            "cna.breakdown.pool",
            record(vec![
                field("allocations", list(allocation.clone(), 0, 6), false),
                field(
                    "cohort",
                    ActionSchema::Text {
                        min_length: 1,
                        max_length: 512,
                    },
                    false,
                ),
            ]),
            false,
        ),
        ("cna.breakdown.close", ActionSchema::Bool, true),
        ("cna.breakdown.loss", ActionSchema::Bool, false),
        (
            "cna.truck.allocation",
            record(vec![
                field("packing", packing, false),
                field("transfers", opaque_orders.clone(), false),
            ]),
            true,
        ),
        ("cna.logistics.arrivals.batch", allocation.clone(), true),
        ("cna.logistics.empty", choice(0), true),
        (
            "cna.logistics.attrition",
            list(allocation.clone(), 0, 6),
            false,
        ),
        (
            "cna.logistics.stores.batch",
            list(allocation.clone(), 0, 6),
            true,
        ),
        ("cna.logistics.water.batch", allocation.clone(), true),
        (
            "cna.logistics.well.allocate.batch",
            allocation.clone(),
            true,
        ),
        ("cna.logistics.distribution.batch", allocation, true),
        ("cna.logistics.cargo_history", draws, false),
        (
            "cna.logistics.coastal.axis",
            list(
                record(vec![
                    field("ship", choice(3), false),
                    field("operation", choice(3), false),
                    field("dump", choice(3), false),
                    field("cargo", amounts(false), false),
                    field("path", list(hex(true), 0, 50), false),
                ]),
                0,
                1024,
            ),
            true,
        ),
        ("cna.logistics.coastal.commonwealth", coastal(), true),
        (
            "cna.logistics.convoy.plan",
            record(vec![field("convoys", combat, false)]),
            true,
        ),
        ("cna.logistics.truck_convoy.batch", opaque_orders, true),
        (
            "cna.air.designation",
            record(
                (0..8)
                    .map(|i| field(&format!("squadron-{i}"), choice(3), false))
                    .collect(),
            ),
            true,
        ),
        (
            "cna.air.tactical_declarations",
            list(
                record(vec![
                    field("mission", choice(4), false),
                    field("light", choice(2), false),
                    field(
                        "target",
                        record(vec![
                            field("hex", hex(false), true),
                            field("scramble_reserve", ActionSchema::Bool, false),
                        ]),
                        false,
                    ),
                    field(
                        "planes",
                        list(
                            record(vec![
                                field("plane", choice(8), false),
                                field("mode", integer(3), false),
                                field("rated_pilot", choice(8), true),
                            ]),
                            1,
                            8,
                        ),
                        false,
                    ),
                ]),
                0,
                8,
            ),
            true,
        ),
        ("cna.air.finite_refit.small", maintenance(8, 3, 2), true),
        ("cna.air.finite_refit.later", maintenance(160, 8, 8), true),
        ("sandbox.choice", choice(3), true),
        ("sandbox.structured", list(unit(), 0, 6), true),
        ("toy.choice", choice(3), false),
    ]
}

#[test]
fn production_shape_answers_match_legacy_bytes_before_budget_hit() {
    for (kind, schema, pass) in production_shapes() {
        let request = request(kind, schema, pass);
        for epoch in [0, 3, u64::MAX] {
            for mode in [Mode::LegalRandom, Mode::PassWhenPossible] {
                for attempt in 0..32 {
                    let old = legacy_answer(&request, epoch, mode, attempt, &NoCandidates);
                    let new = answer(&request, epoch, mode, attempt, &NoCandidates);
                    match old {
                        Ok(old) => {
                            let new = new.unwrap();
                            request.space.check(&new.action).unwrap();
                            assert_eq!(
                                serde_json::to_vec(&new).unwrap(),
                                serde_json::to_vec(&old).unwrap(),
                                "{kind}/{epoch}/{mode:?}/{attempt}"
                            );
                        }
                        Err(old)
                            if old
                                .to_string()
                                .contains("action generation budget exhausted") =>
                        {
                            if pass {
                                assert!(new.unwrap().action.is_null());
                            } else {
                                let error = new.unwrap_err().to_string();
                                assert!(error.contains(kind));
                                assert!(error.contains("no declared pass"));
                            }
                        }
                        Err(old) => assert_eq!(new.unwrap_err().to_string(), old.to_string()),
                    }
                }
            }
        }
    }
}

#[test]
fn high_maximum_coastal_and_later_maintenance_keep_fitting_answers() {
    for (kind, schema) in [
        ("coastal", coastal()),
        ("later-maintenance", maintenance(160, 8, 8)),
    ] {
        let req = request(kind, schema, true);
        let mut fitted_nonpass = 0;
        let mut budget_passes = 0;
        for attempt in 0..256 {
            let old = legacy_answer(&req, 7, Mode::LegalRandom, attempt, &NoCandidates);
            let new = answer(&req, 7, Mode::LegalRandom, attempt, &NoCandidates).unwrap();
            match old {
                Ok(old) => {
                    assert_eq!(
                        serde_json::to_vec(&new).unwrap(),
                        serde_json::to_vec(&old).unwrap()
                    );
                    if !old.action.is_null() {
                        fitted_nonpass += 1;
                    }
                }
                Err(old) => {
                    assert!(
                        old.to_string()
                            .contains("action generation budget exhausted")
                    );
                    assert!(new.action.is_null());
                    budget_passes += 1;
                }
            }
        }
        assert!(
            fitted_nonpass > 0,
            "{kind} fitting answers must not be forced to pass"
        );
        assert!(budget_passes > 0, "{kind} must exercise budget refusal");
    }
}

#[test]
fn fixed_budget_unavailability_passes_or_names_the_kind() {
    let schema = list(ActionSchema::Bool, 5000, 5000);
    for pass in [false, true] {
        let req = request("test.private-budget", schema.clone(), pass);
        for epoch in [0, 7] {
            for mode in [Mode::LegalRandom, Mode::PassWhenPossible] {
                for attempt in 0..64 {
                    let result = answer(&req, epoch, mode, attempt, &NoCandidates);
                    if pass {
                        let response = result.unwrap();
                        assert!(response.action.is_null());
                        assert_eq!(response.decision_id, req.id);
                        assert_eq!(response.decision_revision, req.revision);
                        assert_eq!(response.controller_epoch, epoch);
                    } else {
                        assert_eq!(
                            result.unwrap_err().to_string(),
                            "invalid request: scripted action domain unavailable for test.private-budget: generation budget would be exhausted and no declared pass"
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn node_and_depth_boundaries_match_the_unchanged_generator() {
    let within = request("nodes4096", list(ActionSchema::Bool, 4095, 4095), false);
    assert_eq!(
        answer(&within, 0, Mode::LegalRandom, 0, &NoCandidates).unwrap(),
        legacy_answer(&within, 0, Mode::LegalRandom, 0, &NoCandidates).unwrap()
    );
    let exceeded = request("nodes4097", list(ActionSchema::Bool, 4096, 4096), false);
    assert!(
        answer(&exceeded, 0, Mode::LegalRandom, 0, &NoCandidates)
            .unwrap_err()
            .to_string()
            .contains("nodes4097")
    );
    for depth in [32, 33] {
        let mut schema = ActionSchema::Bool;
        for _ in 0..depth {
            schema = list(schema, 1, 1);
        }
        for pass in [false, true] {
            let req = request("depth", schema.clone(), pass);
            let old = legacy_answer(&req, 0, Mode::LegalRandom, 0, &NoCandidates);
            let new = answer(&req, 0, Mode::LegalRandom, 0, &NoCandidates);
            if let Ok(old) = old {
                assert_eq!(new.unwrap(), old);
            } else if pass {
                assert!(new.unwrap().action.is_null());
            } else {
                assert!(new.unwrap_err().to_string().contains("depth"));
            }
        }
    }
}

#[test]
fn optional_fields_zero_lists_and_full_integer_range_preserve_rng_and_bytes() {
    let req = request(
        "rng-boundaries",
        record(vec![
            field("choice", choice(3), false),
            field("optional", list(ActionSchema::Bool, 0, 7), true),
            field("zero", list(hex(false), 0, 0), false),
            field("unit", unit(), false),
            field("hex", hex(true), false),
            field(
                "wide",
                ActionSchema::Integer {
                    min: i64::MIN,
                    max: i64::MAX,
                },
                false,
            ),
        ]),
        false,
    );
    for attempt in 0..64 {
        let seed: [u8; 32] =
            Sha256::digest(serde_json::to_vec(&(&req, 3_u64, attempt)).unwrap()).into();
        let mut preview = ResourcePreview {
            rng: ChaCha8Rng::from_seed(seed),
            nodes_left: 4096,
        };
        let mut generator = Generator {
            request: &req,
            rng: ChaCha8Rng::from_seed(seed),
            nodes_left: 4096,
            candidates: &NoCandidates,
            attempt,
        };
        assert_eq!(preview.action(&req.space.schema, 0), Ok(()));
        generator.action(&req.space.schema, 0).unwrap();
        assert_eq!(preview.nodes_left, generator.nodes_left);
        assert_eq!(preview.rng.next_u64(), generator.rng.next_u64());
        assert_eq!(
            serde_json::to_vec(
                &answer(&req, 3, Mode::LegalRandom, attempt, &NoCandidates).unwrap()
            )
            .unwrap(),
            serde_json::to_vec(
                &legacy_answer(&req, 3, Mode::LegalRandom, attempt, &NoCandidates).unwrap()
            )
            .unwrap()
        );
    }
}

struct CountingHook {
    calls: Cell<u32>,
    available: bool,
}
impl Candidates for CountingHook {
    fn candidate(&self, _: &DecisionRequest, schema: &ActionSchema, _: u32) -> Option<Value> {
        self.calls.set(self.calls.get() + 1);
        if !self.available {
            return None;
        }
        Some(match schema {
            ActionSchema::Hex { among: None } => json!("C4218"),
            ActionSchema::Path { .. } => json!(["C4219"]),
            ActionSchema::Text { .. } => json!("fixture-cohort"),
            _ => panic!("unexpected hook schema"),
        })
    }
}

#[test]
fn reached_opaque_hooks_run_once_only_in_real_generation_and_keep_errors() {
    for schema in [
        hex(false),
        ActionSchema::Path {
            from: "C4218".into(),
            max_steps: 2,
        },
        ActionSchema::Text {
            min_length: 1,
            max_length: 512,
        },
    ] {
        let req = request(
            "hook",
            record(vec![
                field("prefix", integer(17), false),
                field("opaque", schema, false),
            ]),
            false,
        );
        for available in [false, true] {
            let hook = CountingHook {
                calls: Cell::new(0),
                available,
            };
            let old = legacy_answer(&req, 3, Mode::LegalRandom, 0, &hook);
            assert_eq!(hook.calls.replace(0), 1);
            let new = answer(&req, 3, Mode::LegalRandom, 0, &hook);
            assert_eq!(hook.calls.get(), 1);
            match old {
                Ok(old) => assert_eq!(
                    serde_json::to_vec(&new.unwrap()).unwrap(),
                    serde_json::to_vec(&old).unwrap()
                ),
                Err(old) => assert_eq!(new.unwrap_err().to_string(), old.to_string()),
            }
        }
    }
    let req = request(
        "opaque-before-budget",
        record(vec![
            field("opaque", hex(false), false),
            field("too_large", list(ActionSchema::Bool, 5000, 5000), false),
        ]),
        true,
    );
    let hook = CountingHook {
        calls: Cell::new(0),
        available: true,
    };
    for attempt in 0..32 {
        hook.calls.set(0);
        let old = legacy_answer(&req, 3, Mode::LegalRandom, attempt, &hook);
        let calls = hook.calls.replace(0);
        let new = answer(&req, 3, Mode::LegalRandom, attempt, &hook);
        assert_eq!(hook.calls.get(), calls);
        match old {
            Ok(old) => assert_eq!(new.unwrap(), old),
            Err(old) => assert_eq!(new.unwrap_err().to_string(), old.to_string()),
        }
    }
}

#[test]
fn malformed_domains_defer_before_any_later_budget_refusal() {
    for schema in [
        choice(0),
        ActionSchema::Unit { among: vec![] },
        ActionSchema::Hex {
            among: Some(vec![]),
        },
        ActionSchema::Integer { min: 2, max: 1 },
        list(ActionSchema::Bool, 2, 1),
    ] {
        let req = request(
            "malformed",
            record(vec![
                field("malformed", schema, false),
                field("too_large", list(ActionSchema::Bool, 5000, 5000), false),
            ]),
            true,
        );
        for attempt in 0..32 {
            let old = legacy_answer(&req, 3, Mode::LegalRandom, attempt, &NoCandidates);
            let new = answer(&req, 3, Mode::LegalRandom, attempt, &NoCandidates);
            match old {
                Ok(old) => assert_eq!(new.unwrap(), old),
                Err(old) => assert_eq!(new.unwrap_err().to_string(), old.to_string()),
            }
        }
    }
}

/// Actual public CNA ingress, not a synthetic ActionSchema. This targets the
/// labelled dev finite-consumer domain correction; it grants no exempt rights.
/// Cases: airlog:35.14, airlog:36.5, airlog:38.33, scen:59.36
#[test]
fn actual_air_domain_with_own_planes_and_no_finite_consumer_answers_legally() {
    use cna_core::{
        dice::CampaignRng,
        engine::{Command, Game, evaluate},
    };
    use cna_rules::{Cna, CnaContent, State, air::maintenance::window, seq::Block};

    let content = CnaContent::load(&cna_content::repo_data_dir(), "graziani").unwrap();
    let mut state = State::new(&content).unwrap();
    // Existing public Air fixtures close the unstarted setup before importing
    // its source-backed unassigned pools. No aircraft or consumer is fabricated.
    assert!(state.setup.tasks.is_empty());
    assert!(state.air.squadrons.is_empty());
    state.setup.closed = true;
    // Limit unrelated ground projection work, retaining the complete Air state.
    state.land.units.clear();
    state.logistics.unit_supply.clear();
    state.logistics.truck_pools.clear();
    state.logistics.dumps.clear();
    state.cursor.block = Block::OpStage;
    state.cursor.index = Block::OpStage
        .steps()
        .iter()
        .position(|step| step.anchor == window::TACTICAL)
        .unwrap();
    // Exercise the later-period no-consumer correction, not the first-stage bar.
    state.cursor.op_stage = Some(2);
    assert_ne!(
        state.cursor.op_stage,
        Some(content.scenario.meta.start.opstage)
    );
    state.cursor.entered = false;
    state.turn.player_a = Some(Side::Axis);
    let initial = Game::<Cna> {
        state,
        rng: CampaignRng::from_seed([41; 32]).state(),
    };
    let rules = Cna::dev();
    let opened = evaluate(&rules, &content, &initial, &Command::Advance)
        .unwrap()
        .game;
    assert_eq!(opened.rng, initial.rng);
    assert!(opened.state.air.runtime.sgsus.is_empty());
    let own_planes = opened
        .state
        .air
        .runtime
        .aircraft
        .values()
        .filter(|plane| plane.force == "axis")
        .count();
    assert!(own_planes > 0);
    let pending = rules.pending(&content, &opened.state);
    assert_eq!(pending.len(), 2);
    for side in Side::ALL {
        assert!(pending.iter().any(|request| {
            request.kind == window::KIND && request.seat == SeatId::new(side, Role::Air)
        }));
    }
    let request = pending
        .iter()
        .find(|request| request.seat == SeatId::new(Side::Axis, Role::Air))
        .unwrap();
    assert!(request.space.pass.is_some());
    let context = request.space.context.as_ref().unwrap();
    assert_eq!(context["planes"].as_object().unwrap().len(), own_planes);
    assert!(context["sgsus"].as_object().unwrap().is_empty());
    let ActionSchema::Record { fields } = &request.space.schema else {
        panic!("actual maintenance domain must retain its structured plan");
    };
    // The consumer filter changes cohorts only. Dues still enumerate all own
    // SGSUs; finite draws still enumerate all own dumps with the original limits.
    // In this source-backed fixture there are no SGSUs, so both outer lists are
    // empty without reducing any nested stock domain.
    let dues = &fields
        .iter()
        .find(|field| field.name == "dues")
        .unwrap()
        .schema;
    assert_eq!(dues, &list(choice(0), 0, 0));
    let draws = &fields
        .iter()
        .find(|field| field.name == "draws")
        .unwrap()
        .schema;
    let dump_ids: Vec<_> = context["air_dumps"]
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect();
    let expected_draw = record(vec![
        field(
            "source",
            record(vec![
                field(
                    "source",
                    ActionSchema::Choice {
                        options: vec![ChoiceOption {
                            id: "air_dump".into(),
                            label: "air_dump".into(),
                            detail: None,
                        }],
                    },
                    false,
                ),
                field(
                    "id",
                    ActionSchema::Choice {
                        options: dump_ids
                            .iter()
                            .map(|id| ChoiceOption {
                                id: id.clone(),
                                label: id.clone(),
                                detail: None,
                            })
                            .collect(),
                    },
                    false,
                ),
            ]),
            false,
        ),
        field(
            "amount",
            record(vec![
                field("fuel", integer(i64::from(i32::MAX)), false),
                field("ammo", integer(i64::from(i32::MAX)), false),
                field("stores", integer(i64::from(i32::MAX)), false),
                field("water", integer(i64::from(i32::MAX)), false),
            ]),
            false,
        ),
    ]);
    assert_eq!(
        draws,
        &list(
            record(vec![
                field("servicer", choice(0), false),
                field(
                    "draws",
                    list(expected_draw, 0, u32::try_from(dump_ids.len()).unwrap()),
                    false
                ),
            ]),
            0,
            0
        )
    );
    request.space.check(&Value::Null).unwrap();
    let cohorts = &fields
        .iter()
        .find(|field| field.name == "cohorts")
        .unwrap()
        .schema;
    let ActionSchema::List { item, min, max } = cohorts else {
        panic!("actual cohorts must remain a list");
    };
    assert_eq!((*min, *max), (0, 0));
    let ActionSchema::Record { fields } = item.as_ref() else {
        panic!("cohort item must retain its structured fields");
    };
    let servicer = &fields
        .iter()
        .find(|field| field.name == "servicer")
        .unwrap()
        .schema;
    let ActionSchema::Choice { options } = servicer else {
        panic!("actual cohort servicers must be enumerated");
    };
    assert!(options.is_empty());
    for name in ["fueling", "arming"] {
        let ActionSchema::Record { fields } = &request.space.schema else {
            unreachable!()
        };
        let field = fields.iter().find(|field| field.name == name).unwrap();
        assert!(field.optional);
        let ActionSchema::List { item, min, max } = &field.schema else {
            panic!("finite service must retain its list schema")
        };
        assert_eq!((*min, *max), (0, 0));
        let ActionSchema::Record { fields } = item.as_ref() else {
            panic!("finite service item must retain its structured fields")
        };
        let ActionSchema::Choice { options } = &fields
            .iter()
            .find(|field| field.name == "servicer")
            .unwrap()
            .schema
        else {
            panic!("finite service servicers must be enumerated")
        };
        assert!(options.is_empty());
    }
    let opened_bytes = serde_json::to_vec(&opened).unwrap();
    for epoch in [0, 7] {
        for mode in [Mode::LegalRandom, Mode::PassWhenPossible] {
            for attempt in 0..16 {
                let response = answer(request, epoch, mode, attempt, &NoCandidates).unwrap();
                assert_eq!(
                    response,
                    answer(request, epoch, mode, attempt, &NoCandidates).unwrap()
                );
                assert_eq!(
                    serde_json::to_vec(&response).unwrap(),
                    serde_json::to_vec(
                        &legacy_answer(request, epoch, mode, attempt, &NoCandidates).unwrap()
                    )
                    .unwrap()
                );
                request.space.check(&response.action).unwrap();
                if mode == Mode::PassWhenPossible {
                    assert!(response.action.is_null());
                }
                if !response.action.is_null() {
                    assert_eq!(response.action["cohorts"], json!([]));
                    assert_eq!(response.action["dues"], json!([]));
                    assert_eq!(response.action["draws"], json!([]));
                }
                let accepted = evaluate(&rules, &content, &opened, &Command::Respond(response))
                    .unwrap()
                    .game;
                assert_eq!(accepted.rng, opened.rng);
                assert_eq!(serde_json::to_vec(&opened).unwrap(), opened_bytes);
                assert_eq!(
                    serde_json::to_vec(&accepted.state.logistics).unwrap(),
                    serde_json::to_vec(&opened.state.logistics).unwrap()
                );
                assert_eq!(
                    serde_json::to_vec(&accepted.state.air.runtime.aircraft).unwrap(),
                    serde_json::to_vec(&opened.state.air.runtime.aircraft).unwrap()
                );
                assert_eq!(
                    serde_json::to_vec(&accepted.state.air.runtime.sgsus).unwrap(),
                    serde_json::to_vec(&opened.state.air.runtime.sgsus).unwrap()
                );
                assert!(
                    accepted
                        .state
                        .air
                        .runtime
                        .maintenance
                        .closed
                        .contains(&Side::Axis)
                );
            }
        }
    }
}
