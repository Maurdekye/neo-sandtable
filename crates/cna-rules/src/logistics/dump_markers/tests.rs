use super::*;
use crate::{Cna, CnaContent};
use cna_core::{dice::CampaignRng, engine::Ruleset};
use cna_protocol::Side;
use serde_json::json;
use std::sync::OnceLock;
fn content() -> &'static CnaContent {
    static C: OnceLock<CnaContent> = OnceLock::new();
    C.get_or_init(|| CnaContent::load(&cna_content::repo_data_dir(), "graziani").unwrap())
}
fn init(s: &mut State, seed: u8) -> CampaignRng {
    let mut rng = CampaignRng::from_seed([seed; 32]);
    let mut events = vec![];
    initialize(
        s,
        &mut Cx {
            rng: &mut rng,
            events: &mut events,
        },
    )
    .unwrap();
    assert!(events.is_empty());
    rng
}
/// Cases: land:3.6, land:3.62, airlog:54.11
#[test]
fn shuffled_markers_and_enemy_views_cannot_distinguish_real_and_dummy() {
    let mut saw_different = false;
    let mut first = None;
    for seed in 0..32u8 {
        let mut a = State::new(content()).unwrap();
        for d in a
            .logistics
            .dumps
            .values_mut()
            .filter(|d| d.side == Side::Axis)
        {
            d.location = DumpLocation::Hex {
                hex: "C4020".into(),
            };
            d.active = true;
        }
        let mut b = a.clone();
        for d in b
            .logistics
            .dumps
            .values_mut()
            .filter(|d| d.side == Side::Axis)
        {
            d.dummy = !d.dummy;
            d.supplies = cna_content::scenario::Supplies {
                ammo: 937,
                fuel: 619,
                stores: 331,
                water: 113,
            };
        }
        let ra = init(&mut a, seed);
        let rb = init(&mut b, seed);
        assert_eq!(ra.state(), rb.state());
        let labels = a
            .logistics
            .dumps
            .iter()
            .map(|(id, d)| (id.clone(), d.marker.clone()))
            .collect::<std::collections::BTreeMap<_, _>>();
        assert_eq!(
            labels,
            b.logistics
                .dumps
                .iter()
                .map(|(id, d)| (id.clone(), d.marker.clone()))
                .collect()
        );
        if let Some(prior) = &first {
            saw_different |= prior != &labels
        } else {
            first = Some(labels.clone())
        }
        assert_eq!(
            labels
                .values()
                .collect::<std::collections::BTreeSet<_>>()
                .len(),
            labels.len()
        );
        let enemy = Perspective::Side(Side::Commonwealth);
        assert_eq!(
            serde_json::to_value(Cna::dev().view(content(), &a, enemy)).unwrap(),
            serde_json::to_value(Cna::dev().view(content(), &b, enemy)).unwrap()
        );
        assert_eq!(
            Cna::dev().observe(content(), &a, enemy),
            Cna::dev().observe(content(), &b, enemy)
        );
        for d in a.logistics.dumps.values().filter(|d| d.side == Side::Axis) {
            let public = marker(d, enemy).unwrap();
            assert_eq!(public.id, d.marker);
            let inspected = crate::view::inspect(content(), &a, enemy, &d.marker, false).unwrap();
            let other = crate::view::inspect(content(), &b, enemy, &d.marker, false).unwrap();
            assert_eq!(inspected, other);
            assert_eq!(inspected, json!({"marker":public}));
            assert!(crate::view::inspect(content(), &a, enemy, &d.id, false).is_err());
            assert!(!serde_json::to_string(&public).unwrap().contains(&d.id));
            let owner = crate::view::inspect(
                content(),
                &a,
                Perspective::Side(Side::Axis),
                &d.marker,
                false,
            )
            .unwrap();
            assert_eq!(owner["dump"]["id"], d.id);
        }
    }
    assert!(
        saw_different,
        "assignments must not be derived from internal IDs"
    );
}
/// Cases: land:3.62, airlog:54.11
#[test]
fn checkpoint_reentry_and_later_real_or_dummy_creations_share_one_sequence() {
    let mut s = State::new(content()).unwrap();
    let mut rng = init(&mut s, 44);
    let previous = serde_json::to_value(&s).unwrap();
    let dice = rng.state();
    initialize(
        &mut s,
        &mut Cx {
            rng: &mut rng,
            events: &mut vec![],
        },
    )
    .unwrap();
    assert_eq!(serde_json::to_value(&s).unwrap(), previous);
    assert_eq!(rng.state(), dice);
    s = serde_json::from_value(previous).unwrap();
    let old = s.logistics.next_dump_marker;
    for (id, dummy) in [("later-real", false), ("later-dummy", true)] {
        let label = next_marker(&mut s.logistics).unwrap();
        s.logistics.dumps.insert(
            id.into(),
            Dump {
                id: id.into(),
                marker: label,
                side: Side::Axis,
                location: DumpLocation::Hex {
                    hex: "C4020".into(),
                },
                supplies: Default::default(),
                active: true,
                dummy,
            },
        );
    }
    assert_eq!(
        s.logistics.dumps["later-real"].marker,
        format!("dump-{}", old + 1)
    );
    assert_eq!(
        s.logistics.dumps["later-dummy"].marker,
        format!("dump-{}", old + 2)
    );
    s.logistics.dumps.remove("later-real");
    assert_eq!(
        next_marker(&mut s.logistics).unwrap(),
        format!("dump-{}", old + 3)
    );
    s.logistics.next_dump_marker = u64::MAX;
    assert_eq!(next_marker(&mut s.logistics), Err(SupplyError::Invalid));
}
