use super::*;
use crate::CnaContent;

fn content() -> CnaContent {
    CnaContent::load(&cna_content::repo_data_dir(), "graziani").unwrap()
}

/// Cases: land:4.44a, airlog:38.21
#[test]
fn fuel_uses_exact_printed_alternative_mode() {
    let content = content();
    let aircraft = &content.units.aircraft["cw.b17d"];
    assert_eq!(fuel_points(aircraft, 0).unwrap(), 5);
    assert_eq!(fuel_points(aircraft, 1).unwrap(), 4);
    assert!(matches!(
        fuel_points(aircraft, 2),
        Err(EngineError::Invariant { .. })
    ));
}

/// Cases: airlog:38.21
#[test]
fn missing_fuel_and_malformed_source_never_substitute_another_mode() {
    let content = content();
    // Deliberately damaged trusted source fixture, not a new aircraft rating.
    let mut aircraft = content.units.aircraft["cw.b17d"].clone();
    aircraft.modes[0].fuel_points = None;
    let before = aircraft.clone();
    assert!(matches!(fuel_points(&aircraft, 0),
        Err(EngineError::Unsupported { case, .. }) if case == "airlog:38.21"));
    assert_eq!(fuel_points(&aircraft, 1).unwrap(), 4);
    assert_eq!(aircraft, before);
    aircraft.modes[0].fuel_points = Some(-1);
    assert!(matches!(
        fuel_points(&aircraft, 0),
        Err(EngineError::Invariant { .. })
    ));
    assert!(matches!(
        fuel_points(&aircraft, usize::MAX),
        Err(EngineError::Invariant { .. })
    ));
}

/// Cases: airlog:35.17, airlog:38.33, airlog:38.34, airlog:38.35, airlog:38.38
#[test]
fn all_supported_nationality_assignment_and_roll_domains_match_printed_percentages() {
    let content = content();
    let table = &content.tables.airlog.aircraft_refit;
    // Independent printed Part B percentages, indexed by modified roll1..9.
    let percentages = [100, 80, 70, 60, 50, 40, 40, 33, 33];
    for (nationality, modifier) in [
        (Nationality::Commonwealth, 0),
        (Nationality::German, 1),
        (Nationality::Italian, 2),
    ] {
        for assigned in [true, false] {
            for face in 1..=6 {
                let modified = usize::from(face) + modifier + usize::from(!assigned);
                let successes =
                    squadron_refitted(table, nationality, assigned, 100, Die::new(face).unwrap())
                        .unwrap();
                assert_eq!(successes, percentages[modified - 1]);
                assert!(successes <= 100);
                assert_eq!(
                    squadron_refitted(table, nationality, assigned, 0, Die::new(face).unwrap())
                        .unwrap(),
                    0
                );
            }
        }
    }
}

/// Cases: airlog:38.34, airlog:38.35, airlog:38.38
#[test]
fn fractional_successes_round_up_without_exceeding_attempts() {
    let content = content();
    let table = &content.tables.airlog.aircraft_refit;
    for (face, attempted, expected) in [
        (1, 1, 1),
        (2, 1, 1),
        (2, 5, 4),
        (2, 6, 5),
        (3, 3, 3),
        (4, 3, 2),
        (5, 3, 2),
        (6, 3, 2),
    ] {
        assert_eq!(
            squadron_refitted(
                table,
                Nationality::Commonwealth,
                true,
                attempted,
                Die::new(face).unwrap()
            )
            .unwrap(),
            expected
        );
    }
    // Italian foreign servicing, die6 -> modified9 ->33percent.
    for (attempted, expected) in [(1, 1), (3, 1), (4, 2), (100, 33), (101, 34)] {
        assert_eq!(
            squadron_refitted(
                table,
                Nationality::Italian,
                false,
                attempted,
                Die::new(6).unwrap()
            )
            .unwrap(),
            expected
        );
    }
}

/// Cases: airlog:38.34, airlog:38.35, airlog:38.38
#[test]
fn largest_count_preserves_exact_rounded_results_for_every_printed_percentage() {
    let content = content();
    let table = &content.tables.airlog.aircraft_refit;
    for (face, expected) in [
        (1, 4_294_967_295),
        (2, 3_435_973_836),
        (3, 3_006_477_107),
        (4, 2_576_980_377),
        (5, 2_147_483_648),
        (6, 1_717_986_918),
    ] {
        assert_eq!(
            squadron_refitted(
                table,
                Nationality::Commonwealth,
                true,
                u32::MAX,
                Die::new(face).unwrap()
            )
            .unwrap(),
            expected
        );
    }
    assert_eq!(
        squadron_refitted(
            table,
            Nationality::Italian,
            false,
            u32::MAX,
            Die::new(6).unwrap()
        )
        .unwrap(),
        1_417_339_208
    );
}
