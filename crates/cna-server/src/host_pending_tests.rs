use super::*;

#[test]
fn forced_pending_borrows_only_owner_at_current_commit_and_refuses_stale_root() {
    let directory = tempfile::tempdir().unwrap();
    let pins = Pins {
        rules_profile: "null-policy-test-v1".into(),
        content_hash: "empty".into(),
        engine_version: "policy-test-v1".into(),
    };
    let mut campaign = Campaign::create(
        &directory.path().join("pending.sqlite"),
        NullPolicyRules {
            pass: false,
            responses: Arc::new(AtomicUsize::new(0)),
        },
        (),
        Game {
            state: 0,
            rng: CampaignRng::from_seed([7; 32]).state(),
        },
        CampaignMeta {
            id: "forced-fixture".into(),
            scenario_id: "test".into(),
            rules_profile: pins.rules_profile.clone(),
            title: "Forced fixture".into(),
            seats: vec![],
        },
        pins,
    )
    .unwrap();
    let owner = SeatId::all().next().unwrap();
    let foreign = SeatId::all().nth(1).unwrap();
    let root = published(&campaign).unwrap();
    assert_eq!(
        root.host_pending(&campaign, owner).unwrap(),
        campaign.pending().as_slice()
    );
    assert!(root.host_pending(&campaign, foreign).unwrap().is_empty());
    campaign
        .handover(
            owner,
            Some(ControllerInfo {
                kind: ControllerKind::LlmCli,
                label: "synthetic".into(),
            }),
            Value::Null,
        )
        .unwrap();
    let before = campaign.state_hash().unwrap();
    let rng = campaign.game.rng;
    let revision = campaign.host_commit_revision();
    let pending = campaign.pending();
    assert!(matches!(
        root.host_pending(&campaign, owner),
        Err(cna_seats::host::HostRefusal::IdentityChanged)
    ));
    assert_eq!(campaign.state_hash().unwrap(), before);
    assert_eq!(campaign.game.rng, rng);
    assert_eq!(campaign.host_commit_revision(), revision);
    assert_eq!(campaign.pending(), pending);
    let current = published(&campaign).unwrap();
    assert_eq!(
        current.host_pending(&campaign, owner).unwrap(),
        pending.as_slice()
    );
    assert!(current.host_pending(&campaign, foreign).unwrap().is_empty());
}
