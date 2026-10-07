mod common;

use cna_tables::land::patrol::{
    ObjectiveLoss, PatrolLosses, PatrolReconnaissance, PatrolSurvival,
    ReconnaissanceResult as Recon,
};
use common::{bind_edited, replace_once, tables};

fn loss(killed: i32, captured: i32) -> PatrolLosses {
    PatrolLosses { killed, captured }
}

/// Cases: land:16.33, land:16.6
#[test]
fn native_survival_all_bands_and_recce_modifier() {
    let chart = &tables().land.patrol_survival;
    // Native bands 0-3, 4, 5, 6 read independently of the raw file.
    let expected = [
        loss(0, 0),
        loss(0, 0),
        loss(0, 0),
        loss(0, 1),
        loss(1, 0),
        loss(1, 1),
    ];
    for die in 1..=6 {
        assert_eq!(chart.losses(die, false), Some(expected[(die - 1) as usize]));
        let recce = if die == 1 {
            loss(0, 0)
        } else {
            expected[(die - 2) as usize]
        };
        assert_eq!(chart.losses(die, true), Some(recce));
    }
    for invalid in [i32::MIN, 0, 7, i32::MAX] {
        assert_eq!(chart.losses(invalid, true), None);
    }
}

/// Cases: land:16.5, land:16.7
#[test]
fn native_recon_all_eighteen_cells_and_rule_example() {
    let chart = &tables().land.patrol_reconnaissance;
    let expected = [
        [0, 0, 1],
        [0, 1, 1],
        [1, 1, 2],
        [1, 2, 3],
        [2, 3, 4],
        [3, 4, -1],
    ];
    for die in 1..=6 {
        for points in 1..=3 {
            let n = expected[(die - 1) as usize][(points - 1) as usize];
            assert_eq!(
                chart.revealed(die, points),
                Some(if n == -1 { Recon::All } else { Recon::Units(n) })
            );
        }
    }
    // Rule 16.5's two surviving points and a four reveal two battalion equivalents.
    assert_eq!(chart.revealed(4, 2), Some(Recon::Units(2)));
    for (die, points) in [(0, 1), (7, 1), (1, 0), (1, 4), (i32::MAX, i32::MIN)] {
        assert_eq!(chart.revealed(die, points), None);
    }
}

/// Cases: land:16.34, land:16.8
#[test]
fn native_objective_all_bands_and_eliminated_patrol_exception() {
    let chart = &tables().land.objective_loss;
    for die in 1..=6 {
        let expected = match die {
            1..=4 => loss(0, 0),
            5 => loss(0, 1),
            _ => loss(1, 0),
        };
        assert_eq!(chart.losses(die, false), Some(expected));
        assert_eq!(
            chart.losses(die, true),
            Some(loss(expected.killed + expected.captured, 0))
        );
    }
    for invalid in [i32::MIN, 0, 7, i32::MAX] {
        assert_eq!(chart.losses(invalid, false), None);
    }
}

/// Cases: land:16.6, land:16.8
#[test]
fn malformed_loss_charts_reject_overlap_holes_notes_and_invalid_counts() {
    for (from, to) in [
        ("die_max = 3", "die_max = 4"),
        ("die_max = 3", "die_max = 2"),
        ("die_min = 0", "die_min = -1"),
        ("killed = 0", "killed = 2"),
        ("captured = 0", "captured = -1"),
        ("id = \"recce_modifier\"", "id = \"other\""),
    ] {
        let err = bind_edited::<PatrolSurvival>("land/16.6-", |text| replace_once(text, from, to))
            .unwrap_err();
        assert!(err.to_string().contains("16.6-patrol-survival.toml"));
        assert!(!err.field.is_empty());
    }
    for (from, to) in [
        ("die_min = 1", "die_min = 0"),
        ("die_max = 4", "die_max = 5"),
        ("die_max = 4", "die_max = 3"),
        ("footnotes = [\"patrol_eliminated\"]", "footnotes = []"),
        ("id = \"patrol_eliminated\"", "id = \"other\""),
    ] {
        let err = bind_edited::<ObjectiveLoss>("land/16.8-", |text| replace_once(text, from, to))
            .unwrap_err();
        assert!(err.to_string().contains("16.8-objective-loss.toml"));
    }
}

/// Cases: land:16.7
#[test]
fn malformed_recon_rejects_duplicate_missing_unknown_columns_and_results() {
    for (from, to) in [
        ("die = 1", "die = 2"),
        ("die = 1", "die = 0"),
        ("net_points_1 = 0", "net_points_1 = -1"),
        ("net_points_1 = 0", "net_points_1 = 5"),
        ("net_points_1 = 0", "net_points_4 = 0"),
        ("net_points_1 = 0, ", ""),
        ("net_points_3 = \"all\"", "net_points_3 = \"unknown\""),
        (
            "[[row]]\ndie = 1\nunits_revealed = { net_points_1 = 0, net_points_2 = 0, net_points_3 = 1 }\n",
            "",
        ),
    ] {
        let err =
            bind_edited::<PatrolReconnaissance>("land/16.7-", |text| replace_once(text, from, to))
                .unwrap_err();
        assert!(err.to_string().contains("16.7-patrol-recon.toml"));
    }
}
