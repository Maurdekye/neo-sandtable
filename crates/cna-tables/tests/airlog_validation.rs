//! Known-negative fixtures for omissions that otherwise reach a lookup as a panic or wrong result.
mod common;

use cna_tables::airlog::air::{
    AircraftRefit, CommonwealthPilotArrival, MaltaAvailability, OffMapAirFacilities,
    ReconAxisConvoys, TacAirKill,
};
use cna_tables::airlog::crt::{AaCombat, AirBombardment, Strafing};
use cna_tables::airlog::distance::{AirDistance, LandDistance};
use cna_tables::{Bound, RawTable, TableError};

fn altered<T: Bound>(prefix: &str, edit: impl FnOnce(&mut toml::Table)) -> TableError {
    let (path, text) = common::table_text(prefix);
    let mut raw = RawTable::parse(&path, &text).unwrap();
    edit(&mut raw.body);
    match T::from_raw(&raw) {
        Ok(_) => panic!("broken fixture was accepted for {}", T::ID),
        Err(error) => {
            assert_eq!(error.file, path);
            error
        }
    }
}

#[test]
fn pilot_totals_outside_two_to_twelve_are_rejected() {
    let e = altered::<CommonwealthPilotArrival>("airlog/34.86", |body| {
        let rows = body["row"].as_array_mut().unwrap();
        let mut extra = rows[0].clone();
        extra["dice_total"] = 13.into();
        rows.push(extra);
    });
    assert_eq!(e.field, "row.dice_total");
}

#[test]
fn malta_totals_outside_two_to_twelve_are_rejected() {
    let e = altered::<MaltaAvailability>("airlog/44.42", |body| {
        let rows = body["row"].as_array_mut().unwrap();
        let mut extra = rows[0].clone();
        extra["dice_total"] = 1.into();
        rows.push(extra);
    });
    assert_eq!(e.field, "row.dice_total");
}

#[test]
fn duplicate_pilot_rating_columns_are_rejected() {
    let e = altered::<CommonwealthPilotArrival>("airlog/34.86", |body| {
        body["pilot_ratings"].as_array_mut().unwrap()[1] = 1.into();
    });
    assert_eq!(e.field, "pilot_ratings");
}

#[test]
fn missing_facility_is_rejected_before_lookup() {
    let e = altered::<OffMapAirFacilities>("airlog/36.53", |body| {
        body["row"].as_array_mut().unwrap().pop();
    });
    assert_eq!(e.field, "row");
}

#[test]
fn missing_air_distance_place_is_rejected() {
    let e = altered::<AirDistance>("airlog/37.4-", |body| {
        body["part_b_row"].as_array_mut().unwrap().pop();
    });
    assert_eq!(e.field, "part_b_row");
}

#[test]
fn missing_land_distance_place_cannot_shrink_the_whole_chart() {
    let e = altered::<LandDistance>("airlog/37.42", |body| {
        body["places"].as_array_mut().unwrap().pop();
        body["row"].as_array_mut().unwrap().pop();
    });
    assert_eq!(e.field, "places");
}

#[test]
fn missing_modified_refit_rolls_are_rejected() {
    let e = altered::<AircraftRefit>("airlog/38.37", |body| {
        body["by_squadron"].as_array_mut().unwrap().pop();
    });
    assert_eq!(e.field, "by_squadron.die_range");
}

#[test]
fn empty_convoy_recon_chart_is_rejected_before_lookup() {
    let e = altered::<ReconAxisConvoys>("airlog/42.53", |body| {
        body["row"].as_array_mut().unwrap().clear();
    });
    assert_eq!(e.field, "row");
}

#[test]
fn convoy_recon_last_row_must_be_open_ended() {
    let e = altered::<ReconAxisConvoys>("airlog/42.53", |body| {
        let last = body["row"].as_array_mut().unwrap().last_mut().unwrap();
        last.as_table_mut().unwrap().remove("planes_min");
        last.as_table_mut()
            .unwrap()
            .insert("planes".into(), 8.into());
    });
    assert!(e.field.starts_with("row["));
}

#[test]
fn decreasing_open_ended_kill_threshold_is_rejected() {
    let e = altered::<TacAirKill>("airlog/45.5", |body| {
        let last = body["row"].as_array_mut().unwrap().last_mut().unwrap();
        last["kill_on_roll_at_most"] = 11.into();
    });
    assert_eq!(e.field, "row.kill_on_roll_at_most");
}

#[test]
fn missing_bombardment_block_is_rejected() {
    let e = altered::<AirBombardment>("airlog/41.5", |body| {
        body["block"].as_array_mut().unwrap().pop();
    });
    assert_eq!(e.field, "block");
}

#[test]
fn bombardment_result_unit_must_match_target() {
    let e = altered::<AirBombardment>("airlog/41.5", |body| {
        body["block"].as_array_mut().unwrap()[0]["result_unit"] = "count".into();
    });
    assert!(e.field.ends_with(".result"));
}

#[test]
fn bombardment_percentages_above_one_hundred_are_rejected() {
    let e = altered::<AirBombardment>("airlog/41.5", |body| {
        body["block"].as_array_mut().unwrap()[1]["row"]
            .as_array_mut()
            .unwrap()[0]["result"] = 101.into();
    });
    assert!(e.field.ends_with(".result"));
}

#[test]
fn column_indices_cannot_disagree_with_band_order() {
    let edit = |body: &mut toml::Table| {
        body["column"].as_array_mut().unwrap()[0]["index"] = 2.into();
    };
    assert_eq!(
        altered::<Strafing>("airlog/40.8", edit).field,
        "column[0].index"
    );
    assert_eq!(
        altered::<AirBombardment>("airlog/41.5", edit).field,
        "column[0].index"
    );
    assert_eq!(
        altered::<AaCombat>("airlog/46.3", edit).field,
        "column[0].index"
    );
}
