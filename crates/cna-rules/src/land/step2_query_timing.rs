//! Test-only, opt-in local timing; no game or public-CI telemetry.
use cpu_time::ThreadTime;
use serde::Serialize;
use std::cell::{Cell, RefCell};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::time::Instant;
const BUCKETS: usize = 15;
const DEPTH: usize = 32;
const EVENTS: u64 = 1_000_000;
#[derive(Clone, Copy)]
#[repr(usize)]
pub(crate) enum Bucket {
    InspectTotal,
    ReachableTotal,
    EdgeRun,
    SegmentFuelTotal,
    SegmentLogisticsClone,
    WithdrawDrawsTotal,
    SupplyLogisticsClone,
    NodeCapture,
    NodeRestore,
    #[allow(dead_code)]
    ReachableSerialize,
    Step2PreparedFuelOverlay,
    Step2HistoryWithdrawalOverlay,
    Step2MovementPointTerrain,
    Step2Legality,
    Step2NodeBuild,
}
#[derive(Clone, Copy)]
#[repr(usize)]
pub(crate) enum Counter {
    FrontierPops,
    NeighborAttempts,
    EdgeSuccess,
    Improvements,
}
#[derive(Clone, Copy, Default, Serialize)]
pub(crate) struct Timing {
    calls: u64,
    cpu_ns: u128,
    wall_ns: u128,
    exclusive_cpu_ns: u128,
    exclusive_wall_ns: u128,
}
#[derive(Clone, Copy, Default)]
struct Frame {
    bucket: usize,
    child_cpu: u128,
    child_wall: u128,
}
#[derive(Default)]
struct Record {
    active: bool,
    timings: [Timing; BUCKETS],
    counts: [u64; 4],
    stack: [[Frame; DEPTH]; 2],
    depth: [usize; 2],
    events: u64,
}
thread_local! {
    static RECORD: RefCell<Record> = RefCell::new(Record::default());
    static UNKNOWN: Cell<bool> = const { Cell::new(false) };
}
fn unknown() {
    let _ = UNKNOWN.try_with(|u| u.set(true));
}
fn protected<T>(f: impl FnOnce() -> Option<T>) -> Option<T> {
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(Some(value)) => Some(value),
        _ => {
            unknown();
            None
        }
    }
}
fn event(r: &mut Record) -> Option<()> {
    r.events = r.events.checked_add(1)?;
    (r.events <= EVENTS).then_some(())
}
pub(crate) fn count(counter: Counter) {
    let _ = protected(|| {
        RECORD
            .try_with(|cell| {
                let mut r = cell.try_borrow_mut().ok()?;
                if !r.active {
                    return Some(());
                }
                event(&mut r)?;
                r.counts[counter as usize] = r.counts[counter as usize].checked_add(1)?;
                Some(())
            })
            .ok()
            .flatten()
    });
}
pub(crate) struct Span {
    start: Option<(usize, usize, ThreadTime, Instant)>,
}
pub(crate) fn span(bucket: Bucket) -> Span {
    let start = protected(|| {
        RECORD
            .try_with(|cell| {
                let mut r = cell.try_borrow_mut().ok()?;
                if !r.active {
                    return Some(None);
                }
                event(&mut r)?;
                let channel = usize::from(bucket as usize >= 10);
                let depth = r.depth[channel];
                if depth >= DEPTH {
                    return None;
                }
                let cpu = ThreadTime::try_now().ok()?;
                let wall = Instant::now();
                r.stack[channel][depth] = Frame {
                    bucket: bucket as usize,
                    ..Frame::default()
                };
                r.depth[channel] += 1;
                Some(Some((channel, depth, cpu, wall)))
            })
            .ok()
            .flatten()
    })
    .flatten();
    Span { start }
}
impl Drop for Span {
    fn drop(&mut self) {
        let Some((channel, depth, cpu, wall)) = self.start.take() else {
            return;
        };
        let _ = protected(|| {
            let cpu = cpu.try_elapsed().ok()?.as_nanos();
            let wall = wall.elapsed().as_nanos();
            RECORD
                .try_with(|cell| {
                    let mut r = cell.try_borrow_mut().ok()?;
                    if !r.active || r.depth[channel] != depth + 1 {
                        return None;
                    }
                    let frame = r.stack[channel][depth];
                    let exclusive_cpu = cpu.checked_sub(frame.child_cpu)?;
                    let exclusive_wall = wall.checked_sub(frame.child_wall)?;
                    let t = &mut r.timings[frame.bucket];
                    t.calls = t.calls.checked_add(1)?;
                    t.cpu_ns = t.cpu_ns.checked_add(cpu)?;
                    t.wall_ns = t.wall_ns.checked_add(wall)?;
                    t.exclusive_cpu_ns = t.exclusive_cpu_ns.checked_add(exclusive_cpu)?;
                    t.exclusive_wall_ns = t.exclusive_wall_ns.checked_add(exclusive_wall)?;
                    r.depth[channel] = depth;
                    if depth > 0 {
                        let parent = &mut r.stack[channel][depth - 1];
                        parent.child_cpu = parent.child_cpu.checked_add(cpu)?;
                        parent.child_wall = parent.child_wall.checked_add(wall)?;
                    }
                    Some(())
                })
                .ok()
                .flatten()
        });
    }
}
#[derive(Serialize)]
pub(crate) struct Report {
    valid: bool,
    names: [&'static str; BUCKETS],
    timings: [Timing; BUCKETS],
    measured: [bool; BUCKETS],
    channels: [u8; BUCKETS],
    counter_names: [&'static str; 4],
    counts: [u64; 4],
    events: u64,
}
pub(crate) struct Session {
    started: bool,
}
impl Session {
    pub(crate) fn begin() -> Self {
        let _ = UNKNOWN.try_with(|u| u.set(false));
        let started = protected(|| {
            RECORD
                .try_with(|cell| {
                    let mut r = cell.try_borrow_mut().ok()?;
                    if r.active {
                        return None;
                    }
                    *r = Record {
                        active: true,
                        ..Record::default()
                    };
                    Some(())
                })
                .ok()
                .flatten()
        })
        .is_some();
        Self { started }
    }
    pub(crate) fn finish(mut self) -> Report {
        let value = if self.started {
            protected(|| {
                RECORD
                    .try_with(|cell| {
                        let mut r = cell.try_borrow_mut().ok()?;
                        if !r.active || r.depth != [0, 0] {
                            return None;
                        }
                        r.active = false;
                        Some((r.timings, r.counts, r.events))
                    })
                    .ok()
                    .flatten()
            })
        } else {
            None
        };
        if value.is_some() {
            self.started = false;
        }
        let valid = value.is_some() && UNKNOWN.try_with(|u| !u.get()).unwrap_or(false);
        let (timings, counts, events) = value.unwrap_or_default();
        Report {
            valid,
            names: [
                "InspectTotal",
                "ReachableTotal",
                "EdgeRun",
                "SegmentFuelTotal",
                "SegmentLogisticsClone",
                "WithdrawDrawsTotal",
                "SupplyLogisticsClone",
                "NodeCapture",
                "NodeRestore",
                "ReachableSerialize",
                "Step2PreparedFuelOverlay",
                "Step2HistoryWithdrawalOverlay",
                "Step2MovementPointTerrain",
                "Step2Legality",
                "Step2NodeBuild",
            ],
            timings,
            measured: [
                true, true, true, true, true, true, true, true, true, false, true, true, true,
                true, true,
            ],
            channels: [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 1],
            counter_names: [
                "FrontierPops",
                "NeighborAttempts",
                "EdgeSuccess",
                "Improvements",
            ],
            counts,
            events,
        }
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        if !self.started {
            return;
        }
        let _ = protected(|| {
            RECORD
                .try_with(|cell| {
                    cell.try_borrow_mut().ok()?.active = false;
                    Some(())
                })
                .ok()
                .flatten()
        });
    }
}
pub(crate) fn emit_private(report: &Report) {
    // Telemetry failure never becomes a game/test error. No game Value is serialized.
    let _ = protected(|| {
        use std::io::Write;
        let encoded = serde_json::to_vec(report).ok()?;
        if encoded.len() > 16 * 1024 {
            return None;
        }
        let mut stderr = std::io::stderr().lock();
        stderr.write_all(b"C7_STEP2_DIAG ").ok()?;
        stderr.write_all(&encoded).ok()?;
        stderr.write_all(b"\n").ok()?;
        Some(())
    });
}
