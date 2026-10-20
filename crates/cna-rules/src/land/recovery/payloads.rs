//! Approved transient recovery contracts; field tanks are the first active producer.
//! Only the trusted recovery prepare/finish implementation constructs these values.
//! No payload is persisted, accepted from Respond, or converted into an event.
use crate::{
    State,
    land::breakdown::{Asset, markers::BrokenMarker, pools::PoolAsset},
    logistics::{
        SupplyDemand, SupplyDraw,
        cargo_history::{CargoSite, LotSelection, Parcel, motion::PhysicalTrucks},
        water::WaterStage,
    },
    state::Location,
};
use cna_content::{scenario::Supplies, units::Trucks};
use cna_core::quantity::WaterPoints;
use cna_protocol::Side;
use cna_tables::land::repair::{RepairCost, RepairLocation, RepairVehicle, VehicleState};

/// Pre-dice choice only; it supplies no outcome, credit, or history.
#[derive(Debug, Clone)]
pub(crate) enum ReturnPoolTarget {
    Existing { pool: String },
    CreateAtSite,
}

/// Reuses the actual Unit/Pool asset namespaces, including equipment and cohort.
#[derive(Debug)]
pub(crate) enum RepairAsset {
    Unit(Asset),
    #[allow(dead_code, reason = "Pool repair awaits its vehicle procedure")]
    Pool(PoolAsset),
}

/// One accepted source-bound row. Cost is resolved from content, not the answer.
#[derive(Debug)]
pub(crate) struct RepairAttempt {
    pub(super) marker: String,
    pub(super) site: Location,
    pub(super) asset: RepairAsset,
    pub(super) mode: RepairLocation,
    pub(super) vehicle_state: VehicleState,
    pub(super) vehicle: RepairVehicle,
    pub(super) cost: RepairCost,
    pub(super) demand: SupplyDemand,
}
impl RepairAttempt {
    pub(crate) fn marker(&self) -> &str {
        &self.marker
    }
    pub(crate) fn site(&self) -> &Location {
        &self.site
    }
    pub(crate) fn asset(&self) -> &RepairAsset {
        &self.asset
    }
    pub(crate) fn mode(&self) -> RepairLocation {
        self.mode
    }
    pub(crate) fn vehicle_state(&self) -> VehicleState {
        self.vehicle_state
    }
    pub(crate) fn vehicle(&self) -> RepairVehicle {
        self.vehicle
    }
    pub(crate) fn cost(&self) -> &RepairCost {
        &self.cost
    }
    pub(crate) fn demand(&self) -> SupplyDemand {
        self.demand
    }
}

/// Each allocation is associated with its real repair site. No fake consumer unit.
/// Owner Airlog revalidates canonical rights and actual charges against current State.
#[derive(Debug)]
pub(crate) struct RepairSiteDebit {
    pub(super) site: Location,
    pub(super) demand: SupplyDemand,
    pub(super) draws: Vec<SupplyDraw>,
}
impl RepairSiteDebit {
    pub(crate) fn site(&self) -> &Location {
        &self.site
    }
    pub(crate) fn demand(&self) -> SupplyDemand {
        self.demand
    }
    pub(crate) fn draws(&self) -> &[SupplyDraw] {
        &self.draws
    }
}

/// Complete accepted attempt, including every site, before any associated die.
/// The exact transient snapshot binds opportunity, marker, stocks, histories and
/// rights. It is not a serialized archive, epoch, answer field or funding account.
#[derive(Debug)]
pub(crate) struct PreparedRepair {
    pub(super) actor: Side,
    pub(super) stage: WaterStage,
    pub(super) before: State,
    pub(super) attempts: Vec<RepairAttempt>,
    pub(super) site_debits: Vec<RepairSiteDebit>,
    pub(super) output_choices: Vec<RepairOutputChoice>,
}
impl PreparedRepair {
    pub(crate) fn actor(&self) -> Side {
        self.actor
    }
    pub(crate) fn stage(&self) -> WaterStage {
        self.stage
    }
    pub(crate) fn before(&self) -> &State {
        &self.before
    }
    pub(crate) fn attempts(&self) -> &[RepairAttempt] {
        &self.attempts
    }
    pub(crate) fn site_debits(&self) -> &[RepairSiteDebit] {
        &self.site_debits
    }
    pub(crate) fn output_choices(&self) -> &[RepairOutputChoice] {
        &self.output_choices
    }
}

/// Trusted copy of the validated pre-dice target and complete cargo priority.
#[derive(Debug)]
pub(crate) struct RepairOutputChoice {
    pub(super) marker: String,
    pub(super) target: ReturnPoolTarget,
    pub(super) cargo_priority: Vec<LotSelection>,
}
impl RepairOutputChoice {
    pub(crate) fn marker(&self) -> &str {
        &self.marker
    }
    pub(crate) fn target(&self) -> &ReturnPoolTarget {
        &self.target
    }
    pub(crate) fn cargo_priority(&self) -> &[LotSelection] {
        &self.cargo_priority
    }
}

/// Classification corresponds exactly to the approved Oob receipt variants.
#[derive(Debug)]
pub(crate) enum ReturnedEquipment {
    Tank {
        equipment: String,
    },
    RecceAc {
        equipment: String,
    },
    #[allow(dead_code, reason = "Gun repair is a later vehicle slice")]
    Gun {
        equipment: String,
    },
}
#[derive(Debug)]
pub(crate) struct ReceiptReturn {
    pub(super) equipment: ReturnedEquipment,
    pub(super) points: i32,
}
impl ReceiptReturn {
    pub(crate) fn equipment(&self) -> &ReturnedEquipment {
        &self.equipment
    }
    pub(crate) fn points(&self) -> i32 {
        self.points
    }
}

/// Positive successful counts and actual selected cargo only; never passengers,
/// marker tank fuel, an old funding account or a current-segment entitlement.
#[derive(Debug)]
pub(crate) struct ReturnedTrucks {
    pub(super) marker: String,
    pub(super) target: ReturnPoolTarget,
    pub(super) trucks: Trucks,
    pub(super) physical_cohorts: Vec<PhysicalTrucks>,
    pub(super) cargo: Supplies,
    pub(super) cargo_parcels: Vec<Parcel>,
    pub(super) activity_water: WaterPoints,
}
impl ReturnedTrucks {
    pub(crate) fn marker(&self) -> &str {
        &self.marker
    }
    pub(crate) fn target(&self) -> &ReturnPoolTarget {
        &self.target
    }
    pub(crate) fn trucks(&self) -> Trucks {
        self.trucks
    }
    pub(crate) fn physical_cohorts(&self) -> &[PhysicalTrucks] {
        &self.physical_cohorts
    }
    pub(crate) fn cargo(&self) -> Supplies {
        self.cargo
    }
    pub(crate) fn cargo_parcels(&self) -> &[Parcel] {
        &self.cargo_parcels
    }
    pub(crate) fn activity_water(&self) -> WaterPoints {
        self.activity_water
    }
}

/// Both snapshots are derived AFTER prepay. Successes and junk are disjoint;
/// after retains the exact failed remainder, cargo/reserves and all passengers.
/// A passenger-only marker remains Some even when no vehicles remain.
#[derive(Debug)]
#[allow(
    dead_code,
    reason = "Full outcome partitions are reserved for later repair procedures"
)]
pub(crate) struct MarkerRepairOutcome {
    pub(super) before: BrokenMarker,
    pub(super) after: Option<BrokenMarker>,
    pub(super) repaired_unit_assets: Vec<Asset>,
    pub(super) repaired_pool_assets: Vec<PoolAsset>,
    pub(super) junk_unit_assets: Vec<Asset>,
    pub(super) junk_pool_assets: Vec<PoolAsset>,
    pub(super) receipts: Vec<ReceiptReturn>,
    pub(super) returned_trucks: Option<ReturnedTrucks>,
}
#[allow(
    dead_code,
    reason = "Full outcome accessors are reserved for later repair procedures"
)]
impl MarkerRepairOutcome {
    pub(crate) fn before(&self) -> &BrokenMarker {
        &self.before
    }
    pub(crate) fn after(&self) -> Option<&BrokenMarker> {
        self.after.as_ref()
    }
    pub(crate) fn repaired_unit_assets(&self) -> &[Asset] {
        &self.repaired_unit_assets
    }
    pub(crate) fn repaired_pool_assets(&self) -> &[PoolAsset] {
        &self.repaired_pool_assets
    }
    pub(crate) fn junk_unit_assets(&self) -> &[Asset] {
        &self.junk_unit_assets
    }
    pub(crate) fn junk_pool_assets(&self) -> &[PoolAsset] {
        &self.junk_pool_assets
    }
    pub(crate) fn receipts(&self) -> &[ReceiptReturn] {
        &self.receipts
    }
    pub(crate) fn returned_trucks(&self) -> Option<&ReturnedTrucks> {
        self.returned_trucks.as_ref()
    }
}

/// One actual repair site, not a cross-site pool merge. The complete attempt
/// was prepaid before Land resolves any of these successful/junk partitions.
#[derive(Debug)]
pub(crate) struct ValidatedRepairOutcome {
    pub(super) actor: Side,
    pub(super) stage: WaterStage,
    pub(super) site: Location,
    pub(super) markers: Vec<MarkerRepairOutcome>,
}
impl ValidatedRepairOutcome {
    pub(crate) fn actor(&self) -> Side {
        self.actor
    }
    pub(crate) fn stage(&self) -> WaterStage {
        self.stage
    }
    pub(crate) fn site(&self) -> &Location {
        &self.site
    }
    pub(crate) fn markers(&self) -> &[MarkerRepairOutcome] {
        &self.markers
    }
}

/// Land prepares each group on the CURRENT draft immediately before that pool
/// write, after prepay. Every member has this same target/site/actor. Snapshot
/// binds actual pool, merged stocks, allocation serial/used IDs and marker state.
/// Oob revalidates full merged capacity and uses its unchanged real allocator.
#[derive(Debug)]
pub(crate) struct PreparedSiteTruckReturn {
    pub(super) actor: Side,
    pub(super) stage: WaterStage,
    pub(super) site: Location,
    pub(super) target: ReturnPoolTarget,
    pub(super) before: State,
    pub(super) returns: Vec<ReturnedTrucks>,
}
impl PreparedSiteTruckReturn {
    pub(crate) fn actor(&self) -> Side {
        self.actor
    }
    pub(crate) fn stage(&self) -> WaterStage {
        self.stage
    }
    pub(crate) fn site(&self) -> &Location {
        &self.site
    }
    pub(crate) fn target(&self) -> &ReturnPoolTarget {
        &self.target
    }
    pub(crate) fn before(&self) -> &State {
        &self.before
    }
    pub(crate) fn returns(&self) -> &[ReturnedTrucks] {
        &self.returns
    }
}

/// Exact existing lots moved, using their original stage/spent/ceiling/freshness.
/// The selection names positive transferred quantities; no history value is an
/// actor input. Source/destination holdings and full history Options are in the
/// physical-boundary snapshots below. Leftovers retain original identities.
#[derive(Debug)]
pub(crate) struct RecoveryCargoMove {
    pub(super) source: CargoSite,
    pub(super) destination: CargoSite,
    pub(super) goods: Supplies,
    pub(super) lots: Vec<LotSelection>,
}
impl RecoveryCargoMove {
    pub(crate) fn source(&self) -> &CargoSite {
        &self.source
    }
    pub(crate) fn destination(&self) -> &CargoSite {
        &self.destination
    }
    pub(crate) fn goods(&self) -> Supplies {
        self.goods
    }
    pub(crate) fn lots(&self) -> &[LotSelection] {
        &self.lots
    }
}

/// Exact real physical cohort partition, including partial-child lineage.
/// No FuelFundingAccount/SegmentKey is an output entitlement. Known/Unknown
/// motion comes from actual snapshot entries, never inferred from absence as zero.
#[derive(Debug)]
pub(crate) struct RecoveryTruckMove {
    pub(super) source: CargoSite,
    pub(super) destination: CargoSite,
    pub(super) selected: Vec<PhysicalTrucks>,
    pub(super) remainder: Vec<PhysicalTrucks>,
    pub(super) relocated: Vec<PhysicalTrucks>,
}
impl RecoveryTruckMove {
    pub(crate) fn source(&self) -> &CargoSite {
        &self.source
    }
    pub(crate) fn destination(&self) -> &CargoSite {
        &self.destination
    }
    pub(crate) fn selected(&self) -> &[PhysicalTrucks] {
        &self.selected
    }
    pub(crate) fn remainder(&self) -> &[PhysicalTrucks] {
        &self.remainder
    }
    pub(crate) fn relocated(&self) -> &[PhysicalTrucks] {
        &self.relocated
    }
}

/// Land's validated physical transition, repair OR tow, on the SAME outer draft.
/// before is post-prepay/pre-output (tow: pre-physical); after is after Oob pool
/// writes and Land marker writes, BEFORE Airlog history relocation. Historical
/// records are untouched between these snapshots. Airlog prepares against the
/// actual current after State, checks these exact physical partitions, then owns
/// the history-only delta. An absent history remains Unknown/absent as applicable.
/// These snapshots are finish-local evidence, not new State fields or replay data.
#[derive(Debug)]
pub(crate) struct ValidatedRepairOrTowOutcome {
    pub(super) actor: Side,
    pub(super) stage: WaterStage,
    pub(super) before: State,
    pub(super) after: State,
    pub(super) cargo_moves: Vec<RecoveryCargoMove>,
    pub(super) truck_moves: Vec<RecoveryTruckMove>,
}
impl ValidatedRepairOrTowOutcome {
    pub(crate) fn actor(&self) -> Side {
        self.actor
    }
    pub(crate) fn stage(&self) -> WaterStage {
        self.stage
    }
    pub(crate) fn before(&self) -> &State {
        &self.before
    }
    pub(crate) fn after(&self) -> &State {
        &self.after
    }
    pub(crate) fn cargo_moves(&self) -> &[RecoveryCargoMove] {
        &self.cargo_moves
    }
    pub(crate) fn truck_moves(&self) -> &[RecoveryTruckMove] {
        &self.truck_moves
    }
}
