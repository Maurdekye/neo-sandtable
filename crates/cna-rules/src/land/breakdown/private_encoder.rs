//! Initial unit-window-only private capture. Never a State, event or wire field.
//! Cases: land:21.43
use super::{Asset, Equipment, RolledCheck, allocation_reason::AllocationReason};
use cna_content::units::Trucks;
use cna_core::diagnostic::{PayloadBounds, PrivatePayload};
use std::fmt::Write;

const OUTPUT: usize = 32_768;
const RESERVE: usize = 4_096;
const INPUT: usize = 65_536;
const ROWS: usize = 256;
const ID_BYTES: usize = 128;

// Fixed, non-allocating same-evaluation observations. No State clone or lookup.
// The candidate is a partial passenger partition actually attempted, never a
// reconstructed full LossPlan. Stocks/fuel/cohorts have not yet been read here.
#[derive(Default)]
pub(super) struct Attempt {
    pub reason: Option<AllocationReason>,
    pub passenger: Option<PassengerAttempt>,
}

pub(super) struct PassengerAttempt {
    unit: [u8; ID_BYTES],
    unit_len: Option<usize>,
    old_trucks: Trucks,
    old_transport: Trucks,
    strength: i32,
    working_transport: Trucks,
    broken_transport: Trucks,
    carried: Option<i32>,
    walking: Option<i32>,
    unresolved: Option<i32>,
    passengers: Option<i32>,
    broken_capacity: Option<i32>,
}

impl PassengerAttempt {
    pub(super) fn input(
        unit: &str,
        old_trucks: Trucks,
        old_transport: Trucks,
        strength: i32,
        working_transport: Trucks,
        broken_transport: Trucks,
    ) -> Self {
        let mut id = [0; ID_BYTES];
        let unit_len = if unit.len() <= ID_BYTES {
            id[..unit.len()].copy_from_slice(unit.as_bytes());
            Some(unit.len())
        } else {
            None
        };
        Self {
            unit: id, unit_len, old_trucks, old_transport, strength,
            working_transport, broken_transport, carried: None, walking: None,
            unresolved: None, passengers: None, broken_capacity: None,
        }
    }

    pub(super) fn reached_capacity(
        mut self, carried: i32, walking: i32, unresolved: i32,
        passengers: i32, broken_capacity: i32,
    ) -> Self {
        self.carried = Some(carried);
        self.walking = Some(walking);
        self.unresolved = Some(unresolved);
        self.passengers = Some(passengers);
        self.broken_capacity = Some(broken_capacity);
        self
    }

    fn unit(&self) -> Option<&str> {
        self.unit_len.and_then(|n| std::str::from_utf8(&self.unit[..n]).ok())
    }
}

// All ordinary formatting is constant; these are never formatted into errors.
impl std::fmt::Debug for Attempt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Attempt(<redacted>)")
    }
}
impl std::fmt::Display for Attempt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Debug::fmt(self, f)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum EncodingFailure { Allocation, Bounds }
impl std::fmt::Debug for EncodingFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("EncodingFailure(<redacted>)")
    }
}
impl std::fmt::Display for EncodingFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Debug::fmt(self, f)
    }
}
impl From<PayloadBounds> for EncodingFailure {
    fn from(_: PayloadBounds) -> Self { Self::Bounds }
}

struct Fixed<'a> { bytes: &'a mut [u8], len: usize }
impl Write for Fixed<'_> {
    fn write_str(&mut self, text: &str) -> std::fmt::Result {
        let end = self.len.checked_add(text.len()).ok_or(std::fmt::Error)?;
        let dest = self.bytes.get_mut(self.len..end).ok_or(std::fmt::Error)?;
        dest.copy_from_slice(text.as_bytes());
        self.len = end;
        Ok(())
    }
}
impl Fixed<'_> {
    fn string(&mut self, text: &str) -> std::fmt::Result {
        // Whole UTF-8 field/escape preflight before writing. Bounded IDs only.
        if text.len() > ID_BYTES { return Err(std::fmt::Error); }
        let encoded = text.bytes().try_fold(2usize, |n, b| n.checked_add(match b {
            0..=31 => 6, b'"' | b'\\' => 2, _ => 1,
        })).ok_or(std::fmt::Error)?;
        if encoded > self.bytes.len().saturating_sub(self.len) {
            return Err(std::fmt::Error);
        }
        self.write_str("\"")?;
        // Copy UTF-8 runs as str; no lossy decode, scalar/byte confusion or trim.
        let mut start = 0;
        for (at, b) in text.bytes().enumerate() {
            if b <= 31 || b == b'"' || b == b'\\' {
                self.write_str(&text[start..at])?;
                match b {
                    0..=31 => write!(self, "\\u00{b:02x}")?,
                    b'"' => self.write_str("\\\"")?,
                    _ => self.write_str("\\\\")?,
                }
                start = at + 1;
            }
        }
        self.write_str(&text[start..])?;
        self.write_str("\"")
    }
    fn trucks(&mut self, t: Trucks) -> std::fmt::Result {
        write!(self, "{{\"native_points_lmh\":[{},{},{}],\"physical_trucks_lmh\":[", t.light, t.medium, t.heavy)?;
        for (at, points) in [t.light, t.medium, t.heavy].into_iter().enumerate() {
            if at != 0 { self.write_str(",")?; }
            self.number(points.checked_mul(10).filter(|n| *n >= 0))?;
        }
        self.write_str("]}")
    }
    fn number(&mut self, n: Option<i32>) -> std::fmt::Result {
        match n { Some(n) => write!(self, "{n}"), None => self.write_str("null") }
    }
}

struct Rows {
    bytes: Vec<u8>, examined: usize, input: usize, kept: usize, open: bool,
    truncated: bool,
}
impl Rows {
    fn new() -> Result<Self, EncodingFailure> {
        let mut bytes = Vec::new();
        // Mandatory buffer first; optional allocation refusal retains a complete
        // minimal record and examines no optional rows. Total reserve is bounded.
        bytes.try_reserve_exact(RESERVE).map_err(|_| EncodingFailure::Allocation)?;
        let optional = bytes.try_reserve_exact(OUTPUT).is_ok();
        bytes.extend_from_slice(b"{\"version\":1,\"site\":\"land:21.43/unit-window\",\"result\":\"unresolved\",\"rows\":[");
        Ok(Self { bytes, examined: 0, input: 0, kept: 0, open: optional, truncated: !optional })
    }
    // Must be called before the next source row is read/encoded. No 257th read.
    fn begin(&mut self) -> bool {
        if !self.open { return false; }
        if self.examined == ROWS { self.close(); return false; }
        self.examined += 1;
        true
    }
    fn close(&mut self) { self.open = false; self.truncated = true; }
    fn row(&mut self, make: impl FnOnce(&mut Fixed<'_>) -> std::fmt::Result) -> bool {
        let mut scratch = [0u8; 2048];
        let mut row = Fixed { bytes: &mut scratch, len: 0 };
        if make(&mut row).is_err() { self.close(); return false; }
        let Some(input) = self.input.checked_add(row.len).filter(|n| *n <= INPUT) else {
            self.close(); return false;
        };
        let comma = usize::from(self.kept != 0);
        // One shared optional budget for roles/families, including delimiters.
        let Some(_end) = self.bytes.len().checked_add(row.len).and_then(|n| n.checked_add(comma))
            .filter(|n| *n <= OUTPUT - RESERVE) else { self.close(); return false; };
        if comma != 0 { self.bytes.push(b','); }
        self.bytes.extend_from_slice(&row.bytes[..row.len]);
        // Selected bytes count ONLY admitted canonical rows. An examined row
        // refused by the output cap contributes no selected input bytes.
        self.input = input;
        self.kept += 1;
        true
    }
}

fn asset_row(out: &mut Fixed<'_>, asset: &Asset) -> std::fmt::Result {
    out.write_str("{\"role\":2,\"family\":\"causal_asset\",\"unit\":")?;
    out.string(asset.unit.as_str())?;
    let (kind, physical) = match asset.equipment {
        Equipment::LightTruck => ("light", asset.points.checked_mul(10).filter(|n| *n >= 0)),
        Equipment::MediumTruck => ("medium", asset.points.checked_mul(10).filter(|n| *n >= 0)),
        Equipment::HeavyTruck => ("heavy", asset.points.checked_mul(10).filter(|n| *n >= 0)),
        Equipment::Weapon(_) => ("weapon", None),
        Equipment::ArmoredRecce => ("armored_recce", None),
    };
    write!(out, ",\"equipment\":\"{kind}\",\"native_points\":{},\"physical_trucks\":", asset.points)?;
    out.number(physical)?;
    // Weapon subtype/cohort are explicitly unavailable in this minimal v1.
    out.write_str(",\"weapon_id\":null,\"cohort\":null}")
}

// Encoder reads ONLY the failed-front original asset prefix and the fixed
// observation made by the actual attempted passenger branch. No holdings scan,
// sorting, full maps, unrelated membership fill, second normalization or solve.
// The Result stays private: its adapter must never propagate into game errors.
pub(super) fn encode(outcome: &RolledCheck, attempt: &Attempt) -> Result<PrivatePayload, EncodingFailure> {
    let mut rows = Rows::new()?;
    let mut assets_kept = 0;
    let mut selected_member = false;
    for asset in &outcome.group.assets {
        if !rows.begin() { break; }
        if !rows.row(|out| asset_row(out, asset)) { break; }
        assets_kept += 1;
        if attempt.passenger.as_ref().and_then(PassengerAttempt::unit) == Some(asset.unit.as_str()) {
            selected_member = true;
        }
    }
    // Only this actually observed unit can be selected, and only if the causal
    // prefix admitted it. One unit means no sort/dedup or membership expansion.
    let mut input_kept = false;
    let mut candidate_kept = false;
    if selected_member && let Some(p) = &attempt.passenger {
        if rows.begin() {
            input_kept = rows.row(|out| {
                out.write_str("{\"role\":2,\"family\":\"passenger_input\",\"unit\":")?;
                out.string(p.unit().ok_or(std::fmt::Error)?)?;
                write!(out, ",\"strength_toe\":{},\"old_trucks\":", p.strength)?;
                out.trucks(p.old_trucks)?;
                out.write_str(",\"old_transport\":")?;
                out.trucks(p.old_transport)?;
                out.write_str(",\"carried_toe\":")?; out.number(p.carried)?;
                out.write_str(",\"walking_toe\":")?; out.number(p.walking)?;
                out.write_str("}")
            });
        }
        if rows.begin() {
            candidate_kept = rows.row(|out| {
                out.write_str("{\"role\":3,\"family\":\"passenger_candidate\",\"complete_loss_plan\":false,\"unit\":")?;
                out.string(p.unit().ok_or(std::fmt::Error)?)?;
                out.write_str(",\"working_transport\":")?; out.trucks(p.working_transport)?;
                out.write_str(",\"broken_transport\":")?; out.trucks(p.broken_transport)?;
                out.write_str(",\"unresolved_toe\":")?; out.number(p.unresolved)?;
                out.write_str(",\"broken_passengers_toe\":")?; out.number(p.passengers)?;
                out.write_str(",\"broken_capacity_toe\":")?; out.number(p.broken_capacity)?;
                out.write_str("}")
            });
        }
    }
    let mut minimal = [0u8; RESERVE];
    let mut out = Fixed { bytes: &mut minimal, len: 0 };
    let reason = match attempt.reason {
        None => "null",
        Some(AllocationReason::Passenger(super::allocation_reason::PassengerReason::InvalidInput)) => "\"passenger/invalid_input\"",
        Some(AllocationReason::Passenger(super::allocation_reason::PassengerReason::Arithmetic)) => "\"passenger/arithmetic\"",
        Some(AllocationReason::Passenger(super::allocation_reason::PassengerReason::BrokenPassengersExceedCapacity)) => "\"passenger/broken_passengers_exceed_capacity\"",
    };
    write!(&mut out, "],\"representative_attempt_reason\":{reason},\"examined_rows\":{},\"selected_input_bytes\":{},\"truncated\":{},\"asset_rows_omitted\":{{\"known\":{}}},\"input_available\":{input_kept},\"candidate_available\":{candidate_kept},\"candidate_complete\":false,\"unavailable\":[\"packing_internals\",\"fuel\",\"stocks\",\"water_reserves\",\"cohort_history\",\"account_links\",\"complete_search\"],\"unavailable_rows\":{{\"unknown\":true}},\"complete_explanation\":false}}",
        rows.examined, rows.input, rows.truncated, outcome.group.assets.len() - assets_kept)
        .map_err(|_| EncodingFailure::Bounds)?;
    // Capacity was reserved once fallibly; appending <=OUTPUT cannot allocate.
    rows.bytes.extend_from_slice(&out.bytes[..out.len]);
    PrivatePayload::from_encoded(rows.bytes).map_err(EncodingFailure::from)
}

fn payload_or_empty(result: Result<PrivatePayload, EncodingFailure>) -> PrivatePayload {
    match result {
        Ok(payload) => payload,
        Err(_) => PrivatePayload::empty(),
    }
}

pub(super) fn payload(outcome: &RolledCheck, attempt: &Attempt) -> PrivatePayload {
    payload_or_empty(encode(outcome, attempt))
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::{Category, CheckGroup};
    use std::collections::{BTreeMap, BTreeSet};

    fn outcome(count: usize, id: &str) -> RolledCheck {
        RolledCheck {
            group: CheckGroup {
                category: Category::Truck, bar: 0, column: 0, shift: 0,
                assets: (0..count).map(|_| Asset {
                    unit: id.into(), equipment: Equipment::LightTruck,
                    points: 3, cohort: None,
                }).collect(),
            },
            percent: 10, broken: 1, destination: "C4021".into(),
            origins: BTreeMap::new(), require_origin: BTreeSet::new(),
        }
    }
    fn value(outcome: &RolledCheck, attempt: &Attempt) -> serde_json::Value {
        serde_json::from_slice(encode(outcome, attempt).unwrap().as_bytes()).unwrap()
    }

    #[test]
    fn shared_examined_rows_zero_255_256_257_are_honest() {
        for n in [0, 255, 256, 257] {
            // Small synthetic canonical rows isolate the row boundary from the
            // independent byte cap; real asset rows may hit bytes earlier.
            let mut rows = Rows::new().unwrap();
            for _ in 0..n {
                if !rows.begin() { break; }
                assert!(rows.row(|out| out.write_str("{}")));
            }
            assert_eq!(rows.examined, n.min(ROWS));
            assert_eq!(rows.truncated, n > ROWS);
        }
        let v = value(&outcome(0, "u"), &Attempt::default());
        assert_eq!(v["examined_rows"], 0);
        assert_eq!(v["candidate_available"], false);
        assert!(v["representative_attempt_reason"].is_null());
        assert_eq!(v["complete_explanation"], false);
    }

    #[test]
    fn escaping_128_byte_utf8_and_control_ids_remain_whole() {
        let id = "é".repeat(64);
        let v = value(&outcome(1, &id), &Attempt::default());
        assert_eq!(v["rows"][0]["unit"], id);
        let v = value(&outcome(1, "\0\n\"\\é"), &Attempt::default());
        assert_eq!(v["rows"][0]["unit"], "\0\n\"\\é");
        let v = value(&outcome(1, &"x".repeat(129)), &Attempt::default());
        assert!(v["rows"].as_array().unwrap().is_empty());
        assert_eq!(v["truncated"], true);
        assert_eq!(v["asset_rows_omitted"]["known"], 1);
    }

    #[test]
    fn output_rejected_row_does_not_count_as_selected_input() {
        let mut rows = Rows::new().unwrap();
        let mut selected = 0;
        while rows.bytes.len() < OUTPUT - RESERVE {
            let room = OUTPUT - RESERVE - rows.bytes.len() - usize::from(rows.kept != 0);
            let len = room.min(2048);
            assert!(len >= 2);
            // Ordinary synthetic JSON string rows isolate the exact byte cap.
            let text = format!("\"{}\"", "x".repeat(len - 2));
            assert!(rows.begin());
            assert!(rows.row(|out| out.write_str(&text)));
            selected += len;
            assert_eq!(rows.input, selected);
        }
        assert_eq!(rows.bytes.len(), OUTPUT - RESERVE);
        let before = rows.bytes.clone();
        let kept = rows.kept;
        assert!(rows.begin());
        assert!(!rows.row(|out| out.write_str("{}")));
        assert_eq!(rows.input, selected);
        assert_eq!(rows.kept, kept);
        assert_eq!(rows.examined, kept + 1);
        assert_eq!(rows.bytes, before);
        assert!(rows.truncated);
        assert!(!rows.begin());
    }

    #[test]
    fn encoded_cap_preserves_minimal_reason_and_causal_prefix() {
        let payload = encode(&outcome(257, &"\0".repeat(128)), &Attempt::default()).unwrap();
        assert!(payload.as_bytes().len() <= OUTPUT);
        let v: serde_json::Value = serde_json::from_slice(payload.as_bytes()).unwrap();
        assert_eq!(v["truncated"], true);
        assert!(v["rows"].as_array().unwrap().len() < ROWS);
        assert!(v["selected_input_bytes"].as_u64().unwrap() <= INPUT as u64);
        let selected: usize = v["rows"].as_array().unwrap().iter()
            .map(|row| serde_json::to_vec(row).unwrap().len()).sum();
        assert_eq!(v["selected_input_bytes"].as_u64().unwrap(), selected as u64);
        assert!(v["representative_attempt_reason"].is_null());
        assert_eq!(v["result"], "unresolved"); // never Exhausted
    }

    #[test]
    fn partial_candidate_is_selected_only_from_admitted_causal_members() {
        let input = PassengerAttempt::input(
            "observed", Trucks { light: 3, medium: 6, heavy: 1 },
            Trucks { light: 0, medium: 2, heavy: 0 }, 6,
            Trucks { light: 0, medium: 2, heavy: 0 }, Trucks::default(),
        ).reached_capacity(2, 4, 0, 1, 0);
        let attempt = Attempt {
            reason: Some(AllocationReason::Passenger(
                super::super::allocation_reason::PassengerReason::BrokenPassengersExceedCapacity,
            )), passenger: Some(input),
        }; // explicitly synthetic writer inputs, not a captured State/candidate
        let v = value(&outcome(1, "unrelated"), &attempt);
        assert_eq!(v["candidate_available"], false);
        assert_eq!(v["rows"].as_array().unwrap().len(), 1);
        let v = value(&outcome(1, "observed"), &attempt);
        assert_eq!(v["input_available"], true);
        assert_eq!(v["candidate_available"], true);
        assert_eq!(v["candidate_complete"], false);
        assert_eq!(v["rows"][1]["walking_toe"], 4);
        assert_eq!(v["rows"][2]["broken_passengers_toe"], 1);
        assert_eq!(v["rows"][1]["old_trucks"]["native_points_lmh"], serde_json::json!([3, 6, 1]));
        assert_eq!(v["rows"][1]["old_trucks"]["physical_trucks_lmh"], serde_json::json!([30, 60, 10]));
        // Role3 shares role2's row/byte budget; a large causal prefix leaves no
        // permission to skip it or replace it with lower-priority candidate rows.
        let v = value(&outcome(256, "observed"), &attempt);
        assert!(v["examined_rows"].as_u64().unwrap() <= 256);
        assert_eq!(v["candidate_available"], false);
        assert_eq!(v["truncated"], true);
    }

    #[test]
    fn all_private_reason_and_failure_formatting_is_constant() {
        use super::super::allocation_reason::PassengerReason;
        for reason in [PassengerReason::InvalidInput, PassengerReason::Arithmetic,
            PassengerReason::BrokenPassengersExceedCapacity] {
            let reason = AllocationReason::Passenger(reason);
            assert_eq!(format!("{reason:?}"), "AllocationReason(<redacted>)");
            assert_eq!(format!("{reason}"), "AllocationReason(<redacted>)");
        }
        for e in [EncodingFailure::Allocation, EncodingFailure::Bounds] {
            assert_eq!(format!("{e:?}"), "EncodingFailure(<redacted>)");
            assert_eq!(format!("{e}"), "EncodingFailure(<redacted>)");
        }
        assert_eq!(format!("{:?}", Attempt::default()), "Attempt(<redacted>)");
    }

    #[test]
    fn encoding_refusals_become_total_empty_not_complete_zero_record() {
        for refusal in [EncodingFailure::Allocation, EncodingFailure::Bounds] {
            let p = payload_or_empty(Err(refusal));
            assert!(p.as_bytes().is_empty());
        }
        let p = payload(&outcome(0, "u"), &Attempt::default());
        assert!(!p.as_bytes().is_empty());
        assert!(serde_json::from_slice::<serde_json::Value>(p.as_bytes()).unwrap().is_object());
    }

    #[test]
    fn native_to_physical_overflow_is_unknown_not_zero_or_wrapped() {
        let mut storage = [0u8; 2048];
        let mut out = Fixed { bytes: &mut storage, len: 0 };
        out.trucks(Trucks { light: i32::MAX, medium: -1, heavy: 2 }).unwrap();
        let v: serde_json::Value = serde_json::from_slice(&out.bytes[..out.len]).unwrap();
        assert_eq!(v["native_points_lmh"], serde_json::json!([i32::MAX, -1, 2]));
        assert_eq!(v["physical_trucks_lmh"], serde_json::json!([null, null, 20]));
    }
}
