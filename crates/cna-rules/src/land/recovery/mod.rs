//! Scratch access supplement for land::recovery; UNAPPLIED/UNCOMPILED.
//! Crate visibility permits owner helpers to name accessor result types.
//! payloads' pub(super) fields still restrict construction to recovery.
pub(crate) mod payloads;

pub(crate) use payloads::{
    PreparedRepair, PreparedSiteTruckReturn, ReturnPoolTarget, ReturnedEquipment,
    ValidatedRepairOrTowOutcome, ValidatedRepairOutcome,
};

// Approved buffered plan and current closure storage only.
pub mod plans;
pub use plans::{RecoveryClosureKey, RecoveryProcedure, RecoveryProcedureState, RepairPlan, TowPlan};
