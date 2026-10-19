use super::*;

#[test]
fn actual_campaign_stop_persists_labelled_secondary_and_keeps_primary_only_legacy_text() {
    for with_secondary in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("fatal.sqlite");
        let pins = Pins {
            rules_profile: "null-policy-test-v1".into(),
            content_hash: "empty".into(),
            engine_version: "policy-test-v1".into(),
        };
        let rules = || NullPolicyRules {
            pass: false,
            responses: Arc::new(AtomicUsize::new(0)),
        };
        let mut campaign = Campaign::create(
            &path,
            rules(),
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
            pins.clone(),
        )
        .unwrap();
        let primary = Error::Recovery("backend".into());
        let secondary = Error::Recovery("publication".into());
        let before = campaign.state_hash().unwrap();
        let rng = campaign.game.rng;
        campaign
            .stop_runtime_causes(&primary, with_secondary.then_some(&secondary))
            .unwrap();
        let expected = if with_secondary {
            "primary: campaign recovery mismatch: backend\nsecondary: campaign recovery mismatch: publication"
        } else {
            "campaign recovery mismatch: backend"
        };
        assert!(
            matches!(campaign.status(), CampaignStatus::Stopped { error } if error == expected)
        );
        assert!(campaign.stop_is_durable());
        assert_eq!(campaign.state_hash().unwrap(), before);
        assert_eq!(campaign.game.rng, rng);
        drop(campaign);
        let recovered = Campaign::recover(&path, rules(), (), &pins).unwrap();
        assert!(
            matches!(recovered.status(), CampaignStatus::Stopped { error } if error == expected)
        );
        assert_eq!(recovered.state_hash().unwrap(), before);
        assert_eq!(recovered.game.rng, rng);
    }
}
