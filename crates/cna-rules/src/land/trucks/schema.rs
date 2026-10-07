//! Explicit allocation data uses unrestricted cohort text, with history checked by the handler.
use cna_core::{
    decision::{ActionSchema, FieldSchema},
    ids::UnitId,
};
fn field(name: &str, schema: ActionSchema, optional: bool) -> FieldSchema {
    FieldSchema {
        name: name.into(),
        doc: name.replace('_', " "),
        schema,
        optional,
    }
}
fn record(names: &[&str], schema: ActionSchema, optional: bool) -> ActionSchema {
    ActionSchema::Record {
        fields: names
            .iter()
            .map(|name| field(name, schema.clone(), optional))
            .collect(),
    }
}
/// A finite answer shape; all transport, cargo, fuel and water values retain their printed units.
/// Cases: land:8.56, land:8.95, airlog:49.16, airlog:52.42
pub(crate) fn space(units: Vec<UnitId>) -> ActionSchema {
    let count = ActionSchema::Integer {
        min: 0,
        max: i64::from(i32::MAX),
    };
    let supplies = record(&["ammo", "fuel", "stores", "water"], count.clone(), true);
    let packing = record(&["light", "medium", "heavy"], supplies, true);
    let trucks = record(&["light", "medium", "heavy"], count.clone(), true);
    let unit = ActionSchema::Unit { among: units };
    ActionSchema::Record {
        fields: vec![
            field(
                "transfers",
                ActionSchema::List {
                    min: 0,
                    max: 4096,
                    item: Box::new(ActionSchema::Record {
                        fields: vec![
                            field("from", unit.clone(), false),
                            field("to", unit.clone(), false),
                            field(
                                "cohorts",
                                ActionSchema::List {
                                    min: 1,
                                    max: 4096,
                                    item: Box::new(ActionSchema::Record {
                                        fields: vec![
                                            field(
                                                "id",
                                                ActionSchema::Text {
                                                    min_length: 1,
                                                    max_length: 512,
                                                },
                                                false,
                                            ),
                                            field(
                                                "count",
                                                ActionSchema::Integer {
                                                    min: 1,
                                                    max: i64::from(i32::MAX),
                                                },
                                                false,
                                            ),
                                        ],
                                    }),
                                },
                                false,
                            ),
                            field("cargo", packing.clone(), false),
                            field("tank_fuel_tenths", count.clone(), false),
                            field("activity_water_points", count.clone(), false),
                        ],
                    }),
                },
                false,
            ),
            field(
                "allocations",
                ActionSchema::List {
                    min: 0,
                    max: 4096,
                    item: Box::new(ActionSchema::Record {
                        fields: vec![
                            field("unit", unit, false),
                            field("transport", trucks, false),
                            field("packing", packing, false),
                        ],
                    }),
                },
                false,
            ),
        ],
    }
}
