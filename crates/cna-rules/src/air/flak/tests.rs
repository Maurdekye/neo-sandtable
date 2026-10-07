use super::*;

fn content() -> CnaContent {
    CnaContent::load(&cna_content::repo_data_dir(), "graziani").unwrap()
}
fn attack(group: AaGroup, n: usize, density_applies: bool) -> Attack {
    Attack {
        group,
        planes: (0..n)
            .map(|n| PlaneId(format!("axis.plane.{n:03}")))
            .collect(),
        flak_points: 40,
        density_applies,
    }
}

/// Cases: airlog:46.0, airlog:46.3, airlog:46.4
/// Interpretations: interp:airlog-0005
#[test]
fn chart_group_density_and_separate_rolls_use_original_column() {
    let c = content();
    for (n, shift) in [(12, 0), (23, 0), (24, 1), (35, 1), (36, 2)] {
        let mut a = attack(AaGroup::PlanesOnOtherMissions, n, true);
        // Below the final column: density must change the actual chart lookup,
        // rather than being hidden by saturation at the table's right edge.
        a.flak_points = 9;
        for seed in 0..16 {
            let mut rng = CampaignRng::from_seed([seed; 32]);
            let out = resolve(&c, &mut rng, &a).unwrap();
            assert_eq!(out.column_shift, shift);
            let killed = c
                .tables
                .airlog
                .aa_combat
                .planes(
                    a.group,
                    AaEffect::PlanesDestroyed,
                    a.flak_points,
                    shift,
                    out.destroyed_roll.unwrap(),
                )
                .unwrap();
            let aborted = c
                .tables
                .airlog
                .aa_combat
                .planes(
                    a.group,
                    AaEffect::PlanesAborted,
                    a.flak_points,
                    shift,
                    out.aborted_roll.unwrap(),
                )
                .unwrap();
            assert_eq!(out.destroyed.len(), usize::from(killed).min(n));
            assert_eq!(
                out.aborted.len(),
                usize::from(aborted).min(n - out.destroyed.len())
            );
            assert!(out.destroyed.is_disjoint(&out.aborted));
            assert!(
                out.destroyed
                    .union(&out.aborted)
                    .all(|id| a.planes.contains(id))
            );
        }
    }
    let mut rng = CampaignRng::from_seed([0; 32]);
    let fighter = resolve(
        &c,
        &mut rng,
        &attack(AaGroup::PlanesOnFighterMissions, 36, false),
    )
    .unwrap();
    assert_eq!(fighter.column_shift, 0);
    assert!(fighter.destroyed_roll.is_some());
    assert!(fighter.aborted_roll.is_none() && fighter.aborted.is_empty());
    let other = resolve(
        &c,
        &mut rng,
        &attack(AaGroup::PlanesOnOtherMissions, 36, false),
    )
    .unwrap();
    assert_eq!(other.column_shift, 0);
}

/// Cases: airlog:46.0, airlog:46.3
/// Interpretations: interp:air-0004
#[test]
fn singleton_losses_capped_and_aborts_follow_losses_with_exact_dice_order() {
    let c = content();
    for group in [
        AaGroup::PlanesOnFighterMissions,
        AaGroup::PlanesOnOtherMissions,
    ] {
        for seed in 0..64 {
            let mut rng = CampaignRng::from_seed([seed; 32]);
            let mut expected = rng.clone();
            let first = expected.two_dice_reading();
            let second =
                (group == AaGroup::PlanesOnOtherMissions).then(|| expected.two_dice_reading());
            let out = resolve(&c, &mut rng, &attack(group, 1, false)).unwrap();
            assert_eq!(out.destroyed_roll, Some(first));
            assert_eq!(out.aborted_roll, second);
            assert_eq!(
                rng.state(),
                expected.state(),
                "singleton selection needs no choice die"
            );
            assert!(out.destroyed.len() + out.aborted.len() <= 1);
            let kills = c
                .tables
                .airlog
                .aa_combat
                .planes(group, AaEffect::PlanesDestroyed, 40, 0, first)
                .unwrap();
            if kills > 0 {
                assert_eq!(out.destroyed.len(), 1);
                assert!(out.aborted.is_empty());
            }
        }
    }
}

/// Cases: airlog:46.0, airlog:46.25, airlog:46.26
/// Interpretations: interp:air-0004
#[test]
fn malformed_or_empty_strength_plans_never_spend_rng_and_order_is_canonical() {
    let c = content();
    let base = attack(AaGroup::PlanesOnOtherMissions, 24, true);
    let mut rng = CampaignRng::from_seed([11; 32]);
    let before = rng.state();
    let mut invalids = Vec::new();
    let mut a = base.clone();
    a.planes.push(a.planes[0].clone());
    invalids.push(a);
    let mut a = base.clone();
    a.planes[0].0.clear();
    invalids.push(a);
    let mut a = base.clone();
    a.flak_points = -1;
    invalids.push(a);
    invalids.push(attack(AaGroup::PlanesOnFighterMissions, 1, true));
    for a in invalids {
        assert!(resolve(&c, &mut rng, &a).is_err());
        assert_eq!(rng.state(), before);
    }
    let mut zero = base.clone();
    zero.flak_points = 0;
    for a in [zero, attack(AaGroup::PlanesOnOtherMissions, 0, true)] {
        assert_eq!(resolve(&c, &mut rng, &a).unwrap(), Outcome::default());
        assert_eq!(rng.state(), before);
    }
    let checkpoint = serde_json::to_vec(&before).unwrap();
    let mut restored = CampaignRng::from_state(&serde_json::from_slice(&checkpoint).unwrap());
    let mut reversed = base.clone();
    reversed.planes.reverse();
    assert_eq!(
        resolve(&c, &mut rng, &base).unwrap(),
        resolve(&c, &mut restored, &reversed).unwrap()
    );
    assert_eq!(rng.state(), restored.state());
}

/// Cases: airlog:46.0
/// Interpretations: interp:air-0004
#[test]
fn rejection_index_has_equal_preimages_and_rejects_tail() {
    let mut frequencies = [0; 7];
    for value in 0..35 {
        let mut faces = [
            Die::new(value / 6 + 1).unwrap(),
            Die::new(value % 6 + 1).unwrap(),
        ]
        .into_iter();
        frequencies[index_with(7, || faces.next().unwrap())] += 1;
        assert!(faces.next().is_none());
    }
    assert_eq!(frequencies, [5; 7]);
    let mut faces = [6, 6, 1, 1].map(|n| Die::new(n).unwrap()).into_iter();
    assert_eq!(index_with(7, || faces.next().unwrap()), 0);
    assert!(faces.next().is_none());
    assert_eq!(
        index_with(1, || panic!("no random choice for singleton")),
        0
    );
    let mut rng = CampaignRng::from_seed([15; 32]);
    let mut remaining: Vec<_> = (0..7).map(|n| PlaneId(n.to_string())).collect();
    let destroyed = select(&mut rng, &mut remaining, 3);
    let aborted = select(&mut rng, &mut remaining, 6);
    assert_eq!((destroyed.len(), aborted.len(), remaining.len()), (3, 4, 0));
    assert!(destroyed.is_disjoint(&aborted));
}
