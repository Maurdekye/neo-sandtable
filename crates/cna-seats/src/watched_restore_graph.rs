//! In-memory compaction graph only, not transcript/adoption admission.
//! CLI2.1.293 E1r221547364/R1r221548668 and Aqe219522854 were read
//! directly. Typed inputs still require source-complete writer/binding checks.
use super::restore_parts::{RawRow, check_raw_graph};
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum RowKind {
    User,
    Assistant,
    Other,
}
#[derive(Clone)]
pub struct GraphRow {
    pub raw: RawRow,
    pub kind: RowKind,
}
#[derive(Clone)]
pub struct PreservedMessages {
    pub anchor: Uuid,
    pub uuids: Vec<Uuid>,
    pub all_uuids: Vec<Uuid>,
}
#[derive(Clone)]
pub struct PreservedSegment {
    pub head: Uuid,
    pub anchor: Uuid,
    pub tail: Uuid,
}
#[derive(Clone)]
pub struct Boundary {
    pub uuid: Uuid,
    pub messages: Option<PreservedMessages>,
    pub segment: Option<PreservedSegment>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum GraphRefusal {
    #[error("raw graph has duplicate UUIDs or cyclic edges")]
    Raw,
    #[error("compaction boundary binding is unsupported")]
    Boundary,
    #[error("preserved compaction list is invalid")]
    Preserved,
    #[error("preserved segment cannot be walked without repair")]
    Segment,
    #[error("active compaction graph has unresolved references or cycles")]
    Active,
}

pub struct CompactionGraph {
    rows: Vec<GraphRow>,
    usage_reset: BTreeSet<Uuid>,
    pruned: BTreeSet<Uuid>,
}
impl CompactionGraph {
    pub fn rows(&self) -> &[GraphRow] {
        &self.rows
    }
    /// Mirrors the CLI's in-memory usage zeroing only. Persisted cost-state is
    /// separate and MUST remain the exact last valid record, never recomputed.
    pub fn usage_reset(&self) -> &BTreeSet<Uuid> {
        &self.usage_reset
    }
    pub fn pruned(&self) -> &BTreeSet<Uuid> {
        &self.pruned
    }
}

fn unique(values: &[Uuid]) -> Result<BTreeSet<Uuid>, GraphRefusal> {
    let set: BTreeSet<_> = values.iter().copied().collect();
    if values.len() > super::MAX_RECORDS || set.len() != values.len() {
        return Err(GraphRefusal::Preserved);
    }
    Ok(set)
}

fn segment_list(
    segment: &PreservedSegment,
    rows: &[GraphRow],
    index: &BTreeMap<Uuid, usize>,
) -> Result<Vec<Uuid>, GraphRefusal> {
    let mut seen = BTreeSet::new();
    let mut reverse = Vec::new();
    let mut next = Some(segment.tail);
    while let Some(uuid) = next {
        if !seen.insert(uuid) {
            return Err(GraphRefusal::Segment);
        }
        let row = &rows[*index.get(&uuid).ok_or(GraphRefusal::Segment)?];
        reverse.push(uuid);
        if uuid == segment.head {
            reverse.reverse();
            return Ok(reverse);
        }
        next = row.raw.parent;
    }
    Err(GraphRefusal::Segment)
}

/// Reproduce E1r's most recent preserved-metadata/latest-boundary choice and
/// relink/prune on copies of graph edges. All rows/fragments remain distinct.
/// This neither parses writer metadata nor selects a parked conversation leaf.
/// It explicitly refuses broken walks instead of the CLI's recovery fallback.
pub fn compact(
    rows: &[GraphRow],
    boundaries: &[Boundary],
) -> Result<CompactionGraph, GraphRefusal> {
    check_raw_graph(&rows.iter().map(|row| row.raw.clone()).collect::<Vec<_>>())
        .map_err(|_| GraphRefusal::Raw)?;
    let index: BTreeMap<_, _> = rows
        .iter()
        .enumerate()
        .map(|(i, row)| (row.raw.uuid, i))
        .collect();
    let mut ordered = BTreeMap::new();
    for boundary in boundaries {
        let order = *index.get(&boundary.uuid).ok_or(GraphRefusal::Boundary)?;
        if rows[order].kind != RowKind::Other
            || rows[order].raw.parent.is_some()
            || ordered.insert(order, boundary).is_some()
        {
            return Err(GraphRefusal::Boundary);
        }
    }
    let mut result = CompactionGraph {
        rows: rows.to_vec(),
        usage_reset: BTreeSet::new(),
        pruned: BTreeSet::new(),
    };
    let latest = ordered.last_key_value().map(|(order, _)| *order);
    let preserved_boundary = ordered
        .iter()
        .rev()
        .find(|(_, b)| b.messages.is_some() || b.segment.is_some());
    let (Some(latest), Some((preserved_order, boundary))) = (latest, preserved_boundary) else {
        active(&result.rows)?;
        return Ok(result);
    };
    let mut anchor = None;
    let mut preserved = Vec::new();
    if latest == *preserved_order {
        if let Some(messages) = &boundary.messages {
            let retained = unique(&messages.uuids)?;
            let all = unique(&messages.all_uuids)?;
            if !retained.is_subset(&all)
                || all.iter().any(|uuid| !index.contains_key(uuid))
                || !index.contains_key(&messages.anchor)
                || retained.contains(&messages.anchor)
            {
                return Err(GraphRefusal::Preserved);
            }
            if let Some(segment) = &boundary.segment
                && (segment.anchor != messages.anchor
                    || segment_list(segment, rows, &index)? != messages.uuids)
            {
                return Err(GraphRefusal::Segment);
            }
            anchor = Some(messages.anchor);
            preserved = messages.uuids.clone();
        } else if let Some(segment) = &boundary.segment {
            preserved = segment_list(segment, rows, &index)?;
            if !index.contains_key(&segment.anchor) || preserved.contains(&segment.anchor) {
                return Err(GraphRefusal::Segment);
            }
            anchor = Some(segment.anchor);
        }
    }
    let retained: BTreeSet<_> = preserved.iter().copied().collect();
    if let (Some(anchor), Some(last)) = (anchor, preserved.last().copied()) {
        let mut parent = anchor;
        for uuid in &preserved {
            let row = &mut result.rows[index[uuid]];
            row.raw.parent = Some(parent);
            parent = *uuid;
            if row.kind == RowKind::Assistant {
                result.usage_reset.insert(*uuid);
            }
        }
        let first = preserved[0];
        for row in &mut result.rows {
            if row.raw.parent == Some(anchor) && row.raw.uuid != first {
                row.raw.parent = Some(last);
            }
        }
    }
    for row in &result.rows[..latest] {
        if !retained.contains(&row.raw.uuid) {
            result.pruned.insert(row.raw.uuid);
        }
    }
    result
        .rows
        .retain(|row| !result.pruned.contains(&row.raw.uuid));
    if let Some(last) = preserved.last().copied() {
        for row in &mut result.rows {
            if matches!(row.kind, RowKind::User | RowKind::Assistant)
                && row
                    .raw
                    .parent
                    .is_some_and(|parent| result.pruned.contains(&parent))
            {
                row.raw.parent = Some(last);
            }
        }
    }
    active(&result.rows)?;
    Ok(result)
}

fn active(rows: &[GraphRow]) -> Result<(), GraphRefusal> {
    let graph = check_raw_graph(&rows.iter().map(|row| row.raw.clone()).collect::<Vec<_>>())
        .map_err(|_| GraphRefusal::Active)?;
    if !graph.unresolved_parents().is_empty() {
        return Err(GraphRefusal::Active);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn id(n: u128) -> Uuid {
        Uuid::from_u128(n)
    }
    fn row(n: u128, parent: Option<u128>, kind: RowKind) -> GraphRow {
        GraphRow {
            raw: RawRow {
                uuid: id(n),
                parent: parent.map(id),
                logical_parent: None,
                assistant_message_id: None,
            },
            kind,
        }
    }
    fn fixture() -> (Vec<GraphRow>, Boundary) {
        let rows = vec![
            row(1, None, RowKind::User),
            row(2, Some(1), RowKind::Assistant),
            row(3, Some(2), RowKind::User),
            row(4, None, RowKind::Other),
            row(5, Some(4), RowKind::User),
            row(6, Some(5), RowKind::User),
        ];
        let boundary = Boundary {
            uuid: id(4),
            messages: Some(PreservedMessages {
                anchor: id(5),
                uuids: vec![id(2), id(3)],
                all_uuids: vec![id(2), id(3)],
            }),
            segment: Some(PreservedSegment {
                head: id(2),
                tail: id(3),
                anchor: id(5),
            }),
        };
        (rows, boundary)
    }
    #[test]
    fn source_relink_prune_and_usage_zeroing_do_not_modify_input() {
        let (rows, boundary) = fixture();
        let result = compact(&rows, &[boundary]).unwrap();
        let parents: BTreeMap<_, _> = result
            .rows()
            .iter()
            .map(|r| (r.raw.uuid, r.raw.parent))
            .collect();
        assert_eq!(parents[&id(2)], Some(id(5)));
        assert_eq!(parents[&id(3)], Some(id(2)));
        assert_eq!(parents[&id(6)], Some(id(3)));
        assert_eq!(result.pruned(), &BTreeSet::from([id(1)]));
        assert_eq!(result.usage_reset(), &BTreeSet::from([id(2)]));
        assert_eq!(rows[1].raw.parent, Some(id(1)));
    }
    #[test]
    fn malformed_list_segment_anchor_and_post_relink_cycle_refuse() {
        let (rows, boundary) = fixture();
        let mut bad = boundary.clone();
        bad.messages.as_mut().unwrap().uuids.push(id(2));
        assert!(matches!(
            compact(&rows, &[bad]),
            Err(GraphRefusal::Preserved)
        ));
        let mut bad = boundary.clone();
        bad.segment.as_mut().unwrap().tail = id(1);
        assert!(matches!(compact(&rows, &[bad]), Err(GraphRefusal::Segment)));
        let mut bad = boundary.clone();
        bad.messages.as_mut().unwrap().anchor = id(99);
        assert!(matches!(
            compact(&rows, &[bad]),
            Err(GraphRefusal::Preserved)
        ));
        let mut bad_rows = rows;
        bad_rows[4].raw.parent = Some(id(2));
        assert!(matches!(
            compact(&bad_rows, &[boundary]),
            Err(GraphRefusal::Active)
        ));
    }
    #[test]
    fn later_boundary_without_preserved_state_prunes_older_metadata_without_using_it() {
        let (mut rows, boundary) = fixture();
        rows.push(row(7, None, RowKind::Other));
        rows.push(row(8, Some(7), RowKind::User));
        let later = Boundary {
            uuid: id(7),
            messages: None,
            segment: None,
        };
        let result = compact(&rows, &[boundary, later]).unwrap();
        assert_eq!(
            result.rows().iter().map(|r| r.raw.uuid).collect::<Vec<_>>(),
            vec![id(7), id(8)]
        );
        assert!(result.usage_reset().is_empty());
    }
    #[test]
    fn absent_preserved_metadata_keeps_original_graph_and_missing_active_parent_refuses() {
        let rows = vec![
            row(1, None, RowKind::User),
            row(2, Some(1), RowKind::Assistant),
        ];
        let result = compact(&rows, &[]).unwrap();
        assert_eq!(result.rows().len(), 2);
        assert!(result.pruned().is_empty());
        assert!(matches!(
            compact(&[row(2, Some(99), RowKind::User)], &[]),
            Err(GraphRefusal::Active)
        ));
    }
}
