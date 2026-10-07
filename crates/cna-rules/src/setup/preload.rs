//! Optional initial cargo and troop assignments, sharing the truck chart's exact capacity.
use super::{SetupTask, decisions};
use crate::{
    CnaContent, State,
    logistics::{CargoPacking, validate_packing},
    state::Pending,
    steps::illegal,
};
use cna_content::{scenario::Supplies, units::Trucks};
use cna_core::{
    decision::{ActionSchema, ActionSpace, ChoiceOption, FieldSchema},
    engine::{Cx, EngineError, Rejection},
    ids::{SeatId, UnitId},
};
use cna_protocol::{Role, Side};
use cna_tables::airlog::{supply::SupplyType, trucks::TruckType};
use serde_json::Value;
pub(super) const KIND: &str = "cna.setup.preload";
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Asset {
    Unit { unit: UnitId },
    Pool { pool: String },
}
impl Asset {
    pub fn key(&self) -> String {
        match self {
            Self::Unit { unit } => format!("unit:{unit}"),
            Self::Pool { pool } => format!("pool:{pool}"),
        }
    }
}
fn holdings(state: &State, a: &Asset) -> Result<(Side, Trucks, Trucks, Supplies), EngineError> {
    match a {
        Asset::Unit { unit } => {
            let u = state
                .land
                .units
                .get(unit)
                .ok_or_else(|| invariant("missing initial unit"))?;
            Ok((
                u.side,
                u.trucks,
                u.transport_trucks,
                state
                    .logistics
                    .unit_supply
                    .get(unit)
                    .map_or(Supplies::default(), |s| s.carried),
            ))
        }
        Asset::Pool { pool } => {
            let p = state
                .logistics
                .truck_pools
                .iter()
                .find(|p| &p.id == pool)
                .ok_or_else(|| invariant("missing initial pool"))?;
            Ok((p.side, p.trucks, Trucks::default(), p.cargo))
        }
    }
}
fn invariant(detail: &str) -> EngineError {
    EngineError::Invariant {
        detail: detail.into(),
    }
}
fn can_motorize(content: &CnaContent, a: &Asset) -> bool {
    match a {
        Asset::Unit { unit } => content
            .units
            .units
            .get(unit)
            .and_then(|u| u.class.as_ref())
            .and_then(|c| content.units.classes.get(c))
            .is_some_and(|c| matches!(c.unit_type.as_str(), "infantry" | "anti_air")),
        _ => false,
    }
}
fn menu(
    content: &CnaContent,
    state: &mut State,
    a: Asset,
    cx: &mut Cx<'_>,
) -> Result<(), EngineError> {
    let (side, _, _, _) = holdings(state, &a)?;
    let mut options = vec![
        ChoiceOption {
            id: "done".into(),
            label: "Keep this initial truck assignment".into(),
            detail: None,
        },
        ChoiceOption {
            id: "load".into(),
            label: "Set initial cargo".into(),
            detail: None,
        },
    ];
    if can_motorize(content, &a) {
        options.push(ChoiceOption {
            id: "motorize".into(),
            label: "Assign trucks to troops or AA".into(),
            detail: None,
        });
    }
    let key = a.key();
    decisions::open_task(
        state,
        cx,
        SeatId::new(side, Role::Logistics),
        KIND,
        format!("Optional initial load for {key}."),
        "scen:59.45",
        SetupTask::Preload {
            asset: a,
            operation: "menu".into(),
        },
        ActionSpace::new(ActionSchema::Choice { options })
            .with_pass("Finish this initial truck assignment"),
    );
    Ok(())
}
/// Cases: scen:59.45, airlog:53.11, airlog:54.2
pub(super) fn start(
    content: &CnaContent,
    state: &mut State,
    cx: &mut Cx<'_>,
) -> Result<(), EngineError> {
    if state.setup.preload_started || !state.setup.pools_started || !state.setup.tasks.is_empty() {
        return Ok(());
    }
    state.setup.preload_started = true;
    let mut assets: Vec<_> = state
        .land
        .units
        .values()
        .filter(|u| u.setup_group.is_some() && u.trucks.total() > 0)
        .map(|u| Asset::Unit { unit: u.id.clone() })
        .collect();
    assets.extend(
        state
            .logistics
            .truck_pools
            .iter()
            .filter(|p| p.trucks.total() > 0)
            .map(|p| Asset::Pool { pool: p.id.clone() }),
    );
    for a in assets {
        menu(content, state, a, cx)?;
    }
    Ok(())
}
fn supplies_schema(content: &CnaContent, trucks: i32, kind: TruckType) -> ActionSchema {
    let chart = content.tables.airlog.truck_characteristics.truck(kind);
    ActionSchema::Record {
        fields: [
            ("ammo", SupplyType::Ammo),
            ("fuel", SupplyType::Fuel),
            ("stores", SupplyType::Stores),
            ("water", SupplyType::Water),
        ]
        .into_iter()
        .map(|(name, t)| FieldSchema {
            name: name.into(),
            doc: "Initial supply points (omit for none); all types share this truck capacity"
                .into(),
            optional: true,
            schema: ActionSchema::Integer {
                min: 0,
                max: i64::from(trucks) * i64::from(chart.supply_capacity(t)),
            },
        })
        .collect(),
    }
}
fn open_operation(
    content: &CnaContent,
    state: &mut State,
    a: Asset,
    operation: &str,
    cx: &mut Cx<'_>,
) -> Result<(), EngineError> {
    let (side, trucks, transport, _) = holdings(state, &a)?;
    let fields = [
        ("light", TruckType::Light, trucks.light, transport.light),
        ("medium", TruckType::Medium, trucks.medium, transport.medium),
        ("heavy", TruckType::Heavy, trucks.heavy, transport.heavy),
    ]
    .into_iter()
    .map(|(name, kind, total, used)| FieldSchema {
        name: name.into(),
        doc: format!("{name} truck allocation (omit for none)"),
        optional: true,
        schema: if operation == "load" {
            supplies_schema(content, total - used, kind)
        } else {
            ActionSchema::Integer {
                min: 0,
                max: i64::from(total),
            }
        },
    })
    .collect();
    let key = a.key();
    decisions::open_task(
        state,
        cx,
        SeatId::new(side, Role::Logistics),
        KIND,
        format!("Initial {operation} for {key}; supply totals share capacity with troop trucks."),
        "scen:59.45",
        SetupTask::Preload {
            asset: a,
            operation: operation.into(),
        },
        ActionSpace::new(ActionSchema::Record { fields })
            .with_pass("Cancel this operation and keep the current initial assignment"),
    );
    Ok(())
}
/// Cases: scen:59.45, airlog:53.11, airlog:54.2
pub(super) fn answer(
    content: &CnaContent,
    state: &mut State,
    pending: &Pending,
    a: Asset,
    operation: &str,
    action: &Value,
    cx: &mut Cx<'_>,
) -> Result<(), Rejection> {
    let (side, trucks, transport, cargo) = holdings(state, &a).map_err(Rejection::Engine)?;
    if side != pending.seat.side {
        return Err(illegal("initial truck asset belongs to another side"));
    }
    if operation == "menu" {
        return match action.as_str() {
            Some("done") => Ok(()),
            None if action.is_null() => Ok(()),
            Some("load") => {
                open_operation(content, state, a, "load", cx).map_err(Rejection::Engine)
            }
            Some("motorize") if can_motorize(content, &a) => {
                open_operation(content, state, a, "motorize", cx).map_err(Rejection::Engine)
            }
            _ => Err(illegal("choose an offered initial truck operation")),
        };
    }
    if action.is_null() && pending.space.pass.is_some() {
        return menu(content, state, a, cx).map_err(Rejection::Engine);
    }
    if operation == "load" {
        let packing: CargoPacking = serde_json::from_value(action.clone())
            .map_err(|_| illegal("provide cargo by truck and supply type"))?;
        let total = packing
            .totals()
            .map_err(|_| illegal("invalid initial cargo amounts"))?;
        validate_packing(content, &trucks, &transport, &total, &packing)
            .map_err(|_| illegal("initial cargo exceeds shared truck capacity"))?;
        match &a {
            Asset::Unit { unit } => {
                state
                    .logistics
                    .unit_supply
                    .entry(unit.clone())
                    .or_default()
                    .carried = total
            }
            Asset::Pool { pool } => {
                state
                    .logistics
                    .truck_pools
                    .iter_mut()
                    .find(|p| &p.id == pool)
                    .expect("validated pool")
                    .cargo = total
            }
        }
        state.setup.preload_packing.insert(a.key(), packing);
    } else if operation == "motorize" && can_motorize(content, &a) {
        let obj = action
            .as_object()
            .filter(|o| o.len() == 3)
            .ok_or_else(|| illegal("provide three troop truck counts"))?;
        let n = |key: &str| {
            obj.get(key)
                .and_then(Value::as_i64)
                .and_then(|n| i32::try_from(n).ok())
                .filter(|n| *n >= 0)
                .ok_or_else(|| illegal("truck counts must be nonnegative integers"))
        };
        let new = Trucks {
            light: n("light")?,
            medium: n("medium")?,
            heavy: n("heavy")?,
        };
        let packing = state
            .setup
            .preload_packing
            .get(&a.key())
            .cloned()
            .unwrap_or_default();
        validate_packing(content, &trucks, &new, &cargo, &packing)
            .map_err(|_| illegal("troop assignment leaves insufficient cargo capacity"))?;
        let Asset::Unit { unit } = &a else {
            unreachable!()
        };
        let class = content.units.units[unit]
            .class
            .as_ref()
            .map(|id| &content.units.classes[id])
            .expect("motorization class");
        if class.unit_type == "anti_air" && new.light > 0 {
            return Err(illegal("light trucks cannot motorize AA"));
        }
        state
            .land
            .units
            .get_mut(unit)
            .expect("validated unit")
            .transport_trucks = new;
    } else {
        return Err(illegal("unknown initial truck operation"));
    }
    menu(content, state, a, cx).map_err(Rejection::Engine)
}
