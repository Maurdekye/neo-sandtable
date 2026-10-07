//! Air Game procedures and private operational inventory.
//!
//! Setup aggregates are imported once after setup closes. After import, use
//! [`inventory::update`] for every inventory change; it validates a draft and
//! rebuilds the aggregate mirrors atomically. Runtime IDs never enter enemy
//! disclosures; combat labels will be scoped to their disclosure window.

pub mod facilities;
pub mod inventory;
pub mod sgsu;
pub mod state;

pub mod designation;
/// Trusted finish-only combat calculation; no live phase dispatch yet.
pub mod combat;

/// Trusted target-group calculation; no live flak decision window yet.
pub mod flak;
