//! Public dump identities share one sequence across real and dummy counters.
//! Initial assignments use a private campaign-RNG shuffle, independent of contents and kind.
use super::SupplyError;
use crate::{
    State,
    state::{Dump, DumpLocation, LogisticsState},
};
use cna_core::{
    engine::{Cx, EngineError},
    visibility::Perspective,
};
use cna_protocol::Marker;

fn invariant() -> EngineError {
    EngineError::Invariant {
        detail: "invalid public dump marker sequence".into(),
    }
}
/// Allocate the next label on a transactional draft. Both real and dummy creation use this.
/// Cases: land:3.62, airlog:54.11
pub fn next_marker(state: &mut LogisticsState) -> Result<String, SupplyError> {
    state.next_dump_marker = state
        .next_dump_marker
        .checked_add(1)
        .ok_or(SupplyError::Invalid)?;
    Ok(format!("dump-{}", state.next_dump_marker))
}
fn random_index(cx: &mut Cx<'_>, count: usize) -> usize {
    let count = count as u128;
    let mut range = 1u128;
    let mut digits = 0;
    while range < count {
        range *= 6;
        digits += 1
    }
    let limit = range - range % count;
    loop {
        let mut value = 0u128;
        for _ in 0..digits {
            value = value * 6 + u128::from(cx.rng.d6().value() - 1)
        }
        if value < limit {
            return (value % count) as usize;
        }
    }
}
/// Shuffle all unassigned initial counters together. Neither kind, side, location nor stock
/// enters the assignment. Labels and the next counter persist in checkpoints.
/// Cases: land:3.6, land:3.62, airlog:54.11
pub fn initialize(state: &mut State, cx: &mut Cx<'_>) -> Result<(), EngineError> {
    if state.logistics.dump_markers_initialized {
        return Ok(());
    }
    let mut next = state.logistics.next_dump_marker;
    let mut seen = std::collections::BTreeSet::new();
    for d in state
        .logistics
        .dumps
        .values()
        .filter(|d| !d.marker.is_empty())
    {
        let n = d
            .marker
            .strip_prefix("dump-")
            .and_then(|s| s.parse::<u64>().ok())
            .filter(|n| *n > 0)
            .ok_or_else(invariant)?;
        if !seen.insert(n) {
            return Err(invariant());
        }
        next = next.max(n);
    }
    let mut ids = state
        .logistics
        .dumps
        .iter()
        .filter(|(_, d)| d.marker.is_empty())
        .map(|(id, _)| id.clone())
        .collect::<Vec<_>>();
    if next.checked_add(ids.len() as u64).is_none() {
        return Err(invariant());
    }
    state.logistics.next_dump_marker = next;
    for i in (1..ids.len()).rev() {
        let j = random_index(cx, i + 1);
        ids.swap(i, j);
    }
    for id in ids {
        let marker = next_marker(&mut state.logistics).map_err(|_| invariant())?;
        state
            .logistics
            .dumps
            .get_mut(&id)
            .expect("collected dump")
            .marker = marker;
    }
    state.logistics.dump_markers_initialized = true;
    Ok(())
}
/// Public identity and position disclose neither the internal id nor dummy status.
/// Owner labels may add inventory, keeping the public marker key identical in all views.
/// Cases: land:3.62, airlog:54.11
pub fn marker(dump: &Dump, perspective: Perspective) -> Option<Marker> {
    let DumpLocation::Hex { hex } = &dump.location else {
        return None;
    };
    if !dump.active || dump.marker.is_empty() {
        return None;
    }
    let own = crate::view::sees_side(perspective, dump.side);
    Some(Marker {
        id: dump.marker.clone(),
        kind: "supply_dump".into(),
        hex: hex.to_string(),
        side: Some(dump.side),
        label: Some(if own {
            let s = dump.supplies;
            format!(
                "{} [{}]{}: ammo {}, fuel {}, stores {}, water {}",
                dump.marker,
                dump.id,
                if dump.dummy { " (dummy)" } else { "" },
                s.ammo,
                s.fuel,
                s.stores,
                s.water
            )
        } else {
            dump.marker.clone()
        }),
    })
}
#[cfg(test)]
mod tests;
