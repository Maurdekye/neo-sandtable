#![cfg(test)]
use super::*;
use crate::Cna;
use cna_core::{
    decision::DecisionResponse,
    dice::CampaignRng,
    engine::{Command, Game, Ruleset, evaluate},
    visibility::Perspective,
};
use serde_json::json;
fn content() -> CnaContent {
    CnaContent::load(&cna_content::repo_data_dir(), "graziani").unwrap()
}
/// Cases: scen:59.32, scen:59.33, scen:59.34, scen:59.35, scen:60.32, scen:60.42, scen:60.46, airlog:35.23, airlog:35.26, airlog:36.3
#[test]
fn every_initial_air_asset_ends_in_a_capacity_valid_private_squadron_and_recovery_matches() {
    let c = content();
    let mut state = State::new(&c).unwrap();
    let initial = state.air.forces.clone();
    for u in state.land.units.values_mut() {
        u.location = crate::state::Location::NotArrived;
    }
    state.land.undistributed_trucks.clear();
    state.logistics.dumps.clear();
    state.logistics.truck_pools.clear();
    let mut game = evaluate(
        &Cna::dev(),
        &c,
        &Game {
            state,
            rng: CampaignRng::from_seed([6; 32]).state(),
        },
        &Command::Advance,
    )
    .unwrap()
    .game;
    let mut count = 0;
    while let Some(p) = game
        .state
        .decisions
        .pending
        .iter()
        .find(|p| p.kind == KIND)
        .cloned()
    {
        count += 1;
        assert!(count < 300);
        let ActionSchema::Choice { options } = &p.space.schema else {
            panic!()
        };
        let answer = Command::Respond(DecisionResponse {
            decision_id: p.id.clone(),
            seat: p.seat,
            controller_epoch: 1,
            decision_revision: p.revision,
            idempotency_key: p.id.to_string(),
            action: json!(options[0].id),
            public_explanation: None,
        });
        let checkpoint: Game<Cna> =
            serde_json::from_value(serde_json::to_value(&game).unwrap()).unwrap();
        let a = evaluate(&Cna::dev(), &c, &game, &answer).unwrap();
        let b = evaluate(&Cna::dev(), &c, &checkpoint, &answer).unwrap();
        assert_eq!(
            serde_json::to_value(&a.game).unwrap(),
            serde_json::to_value(&b.game).unwrap()
        );
        game = a.game;
    }
    assert!(!game.state.setup.closed);
    assert!(game.state.setup.air_unavailable.is_empty());
    let catalog = facilities::catalog(&c).unwrap();
    for (force, start) in initial {
        let now = &game.state.air.forces[&force];
        assert_eq!(now.sgsu_available, 0);
        assert!(now.planes.values().all(|p| p.total == 0));
        assert!(now.pilots.values().all(|n| *n == 0));
        let squads: Vec<_> = game
            .state
            .air
            .squadrons
            .values()
            .filter(|s| s.force == force)
            .collect();
        assert_eq!(squads.len(), usize::try_from(start.sgsu_available).unwrap());
        for (aircraft, p) in start.planes {
            let assigned: Vec<_> = squads
                .iter()
                .filter_map(|s| s.planes.get(&aircraft))
                .collect();
            assert_eq!(assigned.iter().map(|p| p.total).sum::<i32>(), p.total);
            assert_eq!(assigned.iter().map(|p| p.ready).sum::<i32>(), p.ready);
            assert_eq!(assigned.iter().map(|p| p.fuelled).sum::<i32>(), p.total);
            assert_eq!(assigned.iter().map(|p| p.armed).sum::<i32>(), p.total);
        }
        for (rating, n) in start.pilots {
            assert_eq!(
                squads
                    .iter()
                    .map(|s| s.pilots.get(&rating).copied().unwrap_or(0))
                    .sum::<i32>(),
                n
            );
        }
        for s in squads {
            assert!(
                s.planes.values().map(|p| p.total).sum::<i32>() <= capacity(&c, &s.nationality)
            );
            let f = catalog
                .facilities
                .iter()
                .find(|f| f.id == s.facility)
                .unwrap();
            assert!(
                s.planes
                    .keys()
                    .all(|a| facilities::compatible(f, c.units.aircraft[a].role == "flying_boat"))
            );
            assert!(
                Cna::dev()
                    .inspect(&c, &game.state, Perspective::Side(s.side), &s.id)
                    .is_ok()
            );
            assert!(
                Cna::dev()
                    .inspect(&c, &game.state, Perspective::Side(s.side.opponent()), &s.id)
                    .is_err()
            );
        }
    }
    for f in &catalog.facilities {
        if let Some(limit) = f.limit {
            assert!(
                game.state
                    .air
                    .squadrons
                    .values()
                    .filter(|s| s.facility == f.id)
                    .count()
                    <= usize::try_from(limit).unwrap()
            );
        }
    }
    for row in rows(&c, "malta") {
        assert_eq!(
            game.state
                .air
                .squadrons
                .values()
                .filter(
                    |s| s.force == "malta" && s.initial_aircraft.as_deref() == Some(&row.aircraft)
                )
                .count(),
            usize::try_from(row.sgsu.unwrap()).unwrap()
        );
    }
    assert!(game.state.air.squadrons.values().any(|s| {
        s.planes.values().map(|p| p.ready).sum::<i32>()
            > c.tables
                .airlog
                .squadron_capacity
                .capacity(if s.nationality == "it" {
                    SquadronKind::ItalianSquadriglia
                } else {
                    SquadronKind::CommonwealthSquadron194041
                })
                .ready
    }));
    let public = Cna::dev().observe(&c, &game.state, Perspective::Side(Side::Commonwealth));
    assert!(
        public["air"]["squadrons"]
            .as_object()
            .unwrap()
            .values()
            .all(|s| s["side"] == json!(Side::Commonwealth))
    );
    assert!(public["air"]["forces"].get("axis").is_none());
}
/// Cases: airlog:35.21, airlog:35.28, scen:60.42
#[test]
fn composition_uses_printed_roles_and_only_the_symmetric_scenario_exception() {
    let c = content();
    let mut squad = AirSquadron {
        id: "test".into(),
        force: "commonwealth".into(),
        side: Side::Commonwealth,
        nationality: "cw".into(),
        facility: "airfield_abbassia".into(),
        initial_aircraft: None,
        planes: BTreeMap::new(),
        pilots: BTreeMap::new(),
    };
    squad.planes.insert(
        "cw.ms406".into(),
        crate::state::PlaneCount {
            total: 1,
            ready: 1,
            fuelled: 1,
            armed: 1,
        },
    );
    assert!(compatible(&c, &squad, "cw.potez_63_11"));
    assert!(!compatible(&c, &squad, "cw.bombay_i"));
    squad.initial_aircraft = Some("cw.ms406".into());
    assert!(!compatible(&c, &squad, "cw.potez_63_11"));
    squad.initial_aircraft = None;
    squad.nationality = "ge".into();
    squad.planes.clear();
    assert!(!compatible(&c, &squad, "it.cr42"));
}
