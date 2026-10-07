use super::*;
use cna_tables::airlog::air::KillThreshold;

fn content() -> CnaContent {
    CnaContent::load(&cna_content::repo_data_dir(), "graziani").unwrap()
}
fn plane(
    content: &CnaContent,
    id: &str,
    aircraft: &str,
    role: CombatRole,
    pilot: u8,
    formation: usize,
    ammo: bool,
) -> Combatant {
    Combatant::from_mode(
        PlaneId(id.into()),
        &content.units.aircraft[aircraft].modes[0],
        CombatContext {
            role,
            pilot_rating: pilot,
            formation_bombers: formation,
            gun_ammunition: ammo,
            night: false,
        },
    )
    .unwrap()
}
fn fighter(content: &CnaContent, id: &str, aircraft: &str, pilot: u8) -> Combatant {
    plane(content, id, aircraft, CombatRole::Fighter, pilot, 0, true)
}
fn group(defender: Combatant, attackers: Vec<Combatant>) -> Engagement {
    Engagement {
        defender,
        attackers,
        return_fire_target: None,
    }
}

/// Cases: airlog:45.0, airlog:45.4, airlog:45.5, airlog:40.15, airlog:45.36
#[test]
fn actual_source_profiles_and_chart_differentials_use_pilots_and_formation() {
    let content = content();
    let cr = fighter(&content, "cr", "it.cr42", 3);
    let hurricane = fighter(&content, "hurricane", "cw.hurricane_i", 2);
    assert_eq!(cr.tacair, 6);
    assert_eq!(hurricane.tacair, 6);
    assert_eq!(cr.differential(&content, &hurricane).unwrap(), -2);
    assert_eq!(hurricane.differential(&content, &cr).unwrap(), 2);
    // Adopted chart reading, rather than the conflicting worked-example threshold.
    assert_eq!(
        content.tables.airlog.tacair_kill.threshold(2),
        KillThreshold::AtMost(16)
    );
    for (count, bonus) in [(0, 0), (5, 0), (6, 1), (17, 1), (18, 2), (100, 2)] {
        let bomber = plane(
            &content,
            "b",
            "ge.ju88d",
            CombatRole::Bomber,
            0,
            count,
            true,
        );
        assert_eq!(bomber.tacair, 5 + bonus);
    }
    let mode = &content.units.aircraft["ge.ju87b"].modes[0];
    let context = CombatContext {
        role: CombatRole::DiveBomber,
        pilot_rating: 0,
        formation_bombers: 6,
        gun_ammunition: true,
        night: false,
    };
    assert!(matches!(
        Combatant::from_mode(PlaneId("d".into()), mode, context),
        Err(EngineError::Invariant { .. })
    ));
    let bf110 = &content.units.aircraft["ge.bf110"].modes[0];
    let night = Combatant::from_mode(
        PlaneId("night".into()),
        bf110,
        CombatContext {
            role: CombatRole::Fighter,
            pilot_rating: 0,
            formation_bombers: 0,
            gun_ammunition: true,
            night: true,
        },
    )
    .unwrap();
    assert_eq!(night.maneuver, 32);
}

/// Cases: airlog:45.0, airlog:45.17, airlog:45.5
#[test]
fn duel_negative_differential_then_pilot_ties_and_kill_cancels_return_fire() {
    let content = content();
    let defender = fighter(&content, "cr", "it.cr42", 3);
    let attacker = fighter(&content, "h", "cw.hurricane_i", 2);
    let plan = [group(defender.clone(), vec![attacker.clone()])];
    let mut found_first_kill = false;
    for byte in 0..64 {
        let mut rng = CampaignRng::from_seed([byte; 32]);
        let result = resolve_wave(&content, &mut rng, &plan).unwrap();
        assert_eq!(result.shots[0].shooter, defender.id);
        assert_eq!(result.shots[0].differential, -2);
        for shot in &result.shots {
            assert_eq!(
                shot.shot_down,
                content
                    .tables
                    .airlog
                    .tacair_kill
                    .kills(shot.differential, shot.reading)
            );
        }
        if result.shots[0].shot_down {
            assert_eq!(result.shots.len(), 1);
            found_first_kill = true;
        }
        let mut expected = CampaignRng::from_seed([byte; 32]);
        for _ in &result.shots {
            expected.two_dice_reading();
        }
        assert_eq!(rng.state(), expected.state());
    }
    assert!(found_first_kill);
    // Equal final differential: greater pilot first, then attacker on pilot tie.
    let mut high = attacker.clone();
    high.tacair = 6;
    high.maneuver = 30;
    high.pilot_rating = 3;
    let mut low = defender.clone();
    low.tacair = 6;
    low.maneuver = 30;
    low.pilot_rating = 2;
    let mut rng = CampaignRng::from_seed([99; 32]);
    let result = resolve_wave(
        &content,
        &mut rng,
        &[group(high.clone(), vec![low.clone()])],
    )
    .unwrap();
    assert_eq!(result.shots[0].shooter, high.id);
    low.pilot_rating = 3;
    let result = resolve_wave(&content, &mut rng, &[group(high, vec![low.clone()])]).unwrap();
    assert_eq!(result.shots[0].shooter, low.id);
}

/// Cases: airlog:45.17, airlog:45.0
#[test]
fn unarmed_plane_keeps_defensive_ratings_and_never_rolls_to_fire() {
    let content = content();
    let armed = fighter(&content, "a", "cw.hurricane_i", 2);
    let mut unarmed = fighter(&content, "d", "it.cr42", 3);
    let differential = armed.differential(&content, &unarmed).unwrap();
    unarmed.gun_ammunition = false;
    assert_eq!(
        armed.differential(&content, &unarmed).unwrap(),
        differential
    );
    let mut rng = CampaignRng::from_seed([12; 32]);
    let result = resolve_wave(
        &content,
        &mut rng,
        &[group(unarmed.clone(), vec![armed.clone()])],
    )
    .unwrap();
    assert_eq!(result.shots.len(), 1);
    assert_eq!(result.shots[0].shooter, armed.id);
    let before = rng.state();
    let mut also_unarmed = armed;
    also_unarmed.gun_ammunition = false;
    let result = resolve_wave(&content, &mut rng, &[group(unarmed, vec![also_unarmed])]).unwrap();
    assert!(result.shots.is_empty());
    assert_eq!(rng.state(), before);
}

/// Cases: airlog:45.0, airlog:45.24, airlog:45.34, airlog:45.19
#[test]
fn outnumbered_fighter_and_dive_bomber_fire_once_nonfighter_fires_at_each() {
    let content = content();
    let attackers = vec![
        fighter(&content, "a", "cw.hurricane_i", 2),
        fighter(&content, "b", "cw.hurricane_i", 2),
    ];
    for (aircraft, role) in [
        ("it.cr42", CombatRole::Fighter),
        ("ge.ju87b", CombatRole::DiveBomber),
        ("ge.ju88d", CombatRole::Bomber),
    ] {
        let defender = plane(&content, "d", aircraft, role, 0, 0, true);
        let one = role != CombatRole::Bomber;
        let plan = Engagement {
            defender: defender.clone(),
            attackers: attackers.clone(),
            return_fire_target: one.then(|| attackers[1].id.clone()),
        };
        let mut rng = CampaignRng::from_seed([4; 32]);
        let result = resolve_wave(&content, &mut rng, &[plan]).unwrap();
        let defender_shots: Vec<_> = result
            .shots
            .iter()
            .filter(|s| s.shooter == defender.id)
            .collect();
        assert_eq!(defender_shots.len(), if one { 1 } else { 2 });
        assert_eq!(result.shots[0].shooter, defender.id);
        if one {
            assert_eq!(result.shots[0].target, attackers[1].id);
        }
        assert!(
            result
                .shots
                .iter()
                .filter(|s| s.shooter != defender.id)
                .all(|s| s.target == defender.id)
        );
        // All defender fire precedes any surviving attacker fire.
        assert!(
            result
                .shots
                .iter()
                .take(defender_shots.len())
                .all(|s| s.shooter == defender.id)
        );
        if let Some(i) = result
            .shots
            .iter()
            .position(|s| s.target == defender.id && s.shot_down)
        {
            assert_eq!(i + 1, result.shots.len());
        }
    }
}

/// Cases: airlog:45.19, airlog:45.24, airlog:45.34, airlog:34.12, airlog:34.13
#[test]
fn malformed_late_matchup_or_missing_source_preserves_rng_and_checkpoint_replays() {
    let content = content();
    let defender = fighter(&content, "d", "it.cr42", 3);
    let attacker = fighter(&content, "a", "cw.hurricane_i", 2);
    let valid = group(defender.clone(), vec![attacker.clone()]);
    let mut rng = CampaignRng::from_seed([13; 32]);
    let before = rng.state();
    assert!(resolve_wave(&content, &mut rng, &[valid.clone(), valid.clone()]).is_err());
    assert_eq!(rng.state(), before);
    let mut bad = valid.clone();
    bad.return_fire_target = Some(PlaneId("unknown".into()));
    assert!(resolve_wave(&content, &mut rng, &[bad]).is_err());
    assert_eq!(rng.state(), before);
    let context = CombatContext {
        role: CombatRole::Fighter,
        pilot_rating: 0,
        formation_bombers: 0,
        gun_ammunition: true,
        night: false,
    };
    let mut missing = content.units.aircraft["cw.hurricane_i"].modes[0].clone();
    missing.tacair = None;
    assert!(
        matches!(Combatant::from_mode(attacker.id.clone(), &missing, context), Err(EngineError::Unsupported { case, .. }) if case == "airlog:34.12")
    );
    missing.tacair = Some(4);
    missing.maneuver = None;
    assert!(
        matches!(Combatant::from_mode(attacker.id, &missing, context), Err(EngineError::Unsupported { case, .. }) if case == "airlog:34.13")
    );
    let serialized = serde_json::to_vec(&before).unwrap();
    let restored = serde_json::from_slice(&serialized).unwrap();
    let mut replay = CampaignRng::from_state(&restored);
    let result = resolve_wave(&content, &mut rng, std::slice::from_ref(&valid)).unwrap();
    assert_eq!(
        result,
        resolve_wave(&content, &mut replay, &[valid]).unwrap()
    );
    assert_eq!(rng.state(), replay.state());
}
