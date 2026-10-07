mod common;

use cna_tables::land::training::{TrainingChart, TrainingSubject as Subject};
use common::{bind_edited, replace_once, tables};

/// Cases: land:17.34, land:17.6
#[test]
fn native_training_chart_all_rows_and_commando_restriction() {
    let chart = &tables().land.training;
    // Independently read from the native five-row chart, including its note.
    let expected = [
        (Subject::InfantryExceptCommando, 3),
        (Subject::TankOrRecce, 6),
        (Subject::Gun, 1),
        (Subject::Commando, 12),
        (Subject::CommonwealthUnitMoralePoint, 6),
    ];
    for (subject, stages) in expected {
        assert_eq!(chart.op_stages(subject), stages);
        assert_eq!(
            subject.requires_assigned_unit(),
            subject == Subject::Commando
        );
    }
}

/// Cases: land:17.6
#[test]
fn malformed_training_chart_rejects_missing_duplicate_unknown_and_invalid_duration() {
    for (from, to) in [
        ("type = \"commando\"", "type = \"gun\""),
        ("type = \"gun\"", "type = \"invented\""),
        ("op_stages = 3", "op_stages = 0"),
        ("op_stages = 3", "op_stages = -1"),
        ("id = \"training_note\"", "id = \"other_note\""),
    ] {
        let err = bind_edited::<TrainingChart>("land/17.6-", |text| replace_once(text, from, to))
            .unwrap_err();
        assert!(err.to_string().contains("17.6-training-chart.toml"));
    }
    let err = bind_edited::<TrainingChart>("land/17.6-", |text| {
        replace_once(text, "[[row]]\ntype = \"gun\"\nop_stages = 1\n", "")
    })
    .unwrap_err();
    assert!(err.to_string().contains("row"));
}
