use super::*;
use crate::seq::{Block, PLAYER_HALF};
use cna_content::scenario::Placement;
use cna_core::dice::CampaignRng;
use std::sync::OnceLock;

fn content() -> &'static CnaContent {
    static C: OnceLock<CnaContent> = OnceLock::new();
    C.get_or_init(|| CnaContent::load(&cna_content::repo_data_dir(), "graziani").unwrap())
}
fn fixture() -> (State, String) {
    let mut state = State::new(content()).unwrap();
    state.cursor.block = Block::PlayerHalf;
    state.cursor.op_stage = Some(1);
    state.cursor.half = Some(Half::A);
    state.cursor.index = PLAYER_HALF
        .iter()
        .position(|step| step.anchor == ANCHOR)
        .unwrap();
    state.turn.player_a = Some(Side::Axis);
    let id = super::super::pools::add_truck_pool(
        &mut state.logistics,
        None,
        Side::Axis,
        Placement::Hex {
            hex: "C4020".into(),
        },
        Some(Location::Hex {
            hex: "C4020".into(),
        }),
        Trucks {
            light: 2,
            medium: 3,
            heavy: 0,
        },
        Supplies::default(),
    )
    .unwrap();
    (state, id)
}

/// Cases: airlog:53.25, land:3.6
#[test]
fn owner_inspect_reports_unknown_and_mixed_without_numeric_fallback_or_seed() {
    let (mut s, id) = fixture();
    let before = serde_json::to_value(&s).unwrap();
    let report = own_report(content(), &s, Side::Axis, &id).unwrap();
    assert_eq!(report["timing"], "unknown");
    assert!(report.get("spent_cp_quarters").is_none());
    assert_eq!(serde_json::to_value(&s).unwrap(), before);
    pool_fuel::seed_created_pool(&mut s, &id).unwrap();
    // A legitimate mixed current history retains real canonical fuel identities.
    // Positive physical CP without any funding record is trusted corruption.
    pool_fuel::spend_pool_segment_fuel(content(), &mut s, &id, 0).unwrap();
    let entry = s
        .logistics
        .cargo_history
        .motion
        .entries
        .iter_mut()
        .find(|e| e.site == CargoSite::Pool(id.clone()))
        .unwrap();
    entry.cohorts[1].spent_cp_quarters = 4;
    let before = serde_json::to_value(&s).unwrap();
    let report = own_report(content(), &s, Side::Axis, &id).unwrap();
    assert_eq!(report["timing"], "mixed");
    assert!(report.get("spent_cp_quarters").is_none());
    assert_eq!(serde_json::to_value(&s).unwrap(), before);
}

/// Cases: airlog:53.25, land:3.6
#[test]
fn own_report_corruption_stays_invariant_and_foreign_missing_are_identical() {
    let (mut s, id) = fixture();
    pool_fuel::seed_created_pool(&mut s, &id).unwrap();
    s.logistics
        .cargo_history
        .motion
        .entries
        .iter_mut()
        .find(|e| e.site == CargoSite::Pool(id.clone()))
        .unwrap()
        .cohorts[0]
        .count += 1;
    let before = serde_json::to_value(&s).unwrap();
    assert!(matches!(
        own_report(content(), &s, Side::Axis, &id),
        Err(Rejection::Engine(EngineError::Invariant { .. }))
    ));
    assert_eq!(
        serde_json::to_value(own_report(content(), &s, Side::Commonwealth, &id).unwrap_err())
            .unwrap(),
        serde_json::to_value(own_report(content(), &s, Side::Commonwealth, "missing").unwrap_err())
            .unwrap()
    );
    assert_eq!(serde_json::to_value(&s).unwrap(), before);
}

/// Cases: airlog:48.0, airlog:53.12, land:3.6
#[test]
fn explicit_empty_list_opens_once_buffers_only_and_checkpoint_closes_once() {
    for strict in [false, true] {
        let (mut s, _) = fixture();
        let mut rng = CampaignRng::from_seed([4; 32]);
        let mut events = vec![];
        finish(
            content(),
            &mut s,
            strict,
            &mut Cx {
                rng: &mut rng,
                events: &mut events,
            },
        )
        .unwrap();
        assert_eq!(s.decisions.pending.len(), 1);
        assert_eq!(s.decisions.pending[0].kind, KIND);
        let opened = serde_json::to_value(&s).unwrap();
        let log = serde_json::to_value(&events).unwrap();
        let dice = rng.state();
        finish(
            content(),
            &mut s,
            strict,
            &mut Cx {
                rng: &mut rng,
                events: &mut events,
            },
        )
        .unwrap();
        assert_eq!(serde_json::to_value(&s).unwrap(), opened);
        assert_eq!(serde_json::to_value(&events).unwrap(), log);
        let pending = s.decisions.pending.remove(0);
        let before = serde_json::to_value(&s).unwrap();
        answer(
            content(),
            &mut s,
            &pending,
            &json!([]),
            strict,
            &mut Cx {
                rng: &mut rng,
                events: &mut events,
            },
        )
        .unwrap();
        let accepted = serde_json::to_value(&s).unwrap();
        let mut normalized = accepted.clone();
        normalized["logistics"]["truck_convoy"]["submitted"] = Value::Null;
        assert_eq!(normalized, before);
        assert_eq!(serde_json::to_value(&events).unwrap(), log);
        assert_eq!(rng.state(), dice);
        assert!(
            answer(
                content(),
                &mut s,
                &pending,
                &json!([]),
                strict,
                &mut Cx {
                    rng: &mut rng,
                    events: &mut events
                }
            )
            .is_err()
        );
        assert_eq!(serde_json::to_value(&s).unwrap(), accepted);
        let mut recovered: State = serde_json::from_value(accepted).unwrap();
        let mut recovered_rng = rng.clone();
        let mut recovered_events = vec![];
        finish(
            content(),
            &mut s,
            strict,
            &mut Cx {
                rng: &mut rng,
                events: &mut events,
            },
        )
        .unwrap();
        finish(
            content(),
            &mut recovered,
            strict,
            &mut Cx {
                rng: &mut recovered_rng,
                events: &mut recovered_events,
            },
        )
        .unwrap();
        assert_eq!(
            serde_json::to_value(&s).unwrap(),
            serde_json::to_value(&recovered).unwrap()
        );
        assert!(s.logistics.truck_convoy.resolved);
        assert_eq!(s.cursor.anchor(), ANCHOR);
        assert_eq!(rng.state(), dice);
        let closed = serde_json::to_value(&s).unwrap();
        finish(
            content(),
            &mut s,
            strict,
            &mut Cx {
                rng: &mut rng,
                events: &mut events,
            },
        )
        .unwrap();
        assert_eq!(serde_json::to_value(&s).unwrap(), closed);
    }
}

/// Cases: airlog:53.12
#[test]
fn serialized_progress_rejects_invalid_keys_duplicate_pools_and_detached_wait() {
    let (s, id) = fixture();
    let key = key(&s).unwrap();
    let valid = json!({"key":key,"submitted":[{"operation":"move","pool":id,"path":[]}],
        "next_order":1,"waiting_pool":id,"resolved":false});
    assert!(serde_json::from_value::<TruckConvoyProcedure>(valid.clone()).is_ok());
    for change in ["duplicate", "index", "wait", "resolved", "key"] {
        let mut bad = valid.clone();
        match change {
            "duplicate" => {
                let row = bad["submitted"][0].clone();
                bad["submitted"].as_array_mut().unwrap().push(row);
            }
            "index" => bad["next_order"] = json!(2),
            "wait" => bad["waiting_pool"] = json!("different"),
            "resolved" => bad["resolved"] = json!(true),
            _ => bad["key"] = Value::Null,
        }
        assert!(
            serde_json::from_value::<TruckConvoyProcedure>(bad).is_err(),
            "{change}"
        );
    }
}
