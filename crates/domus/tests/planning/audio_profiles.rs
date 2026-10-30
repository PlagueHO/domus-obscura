use super::*;
use domus::domain::{AUDIO_POLICY_VERSION, AudioPolicy, AudioProfile, AudioSelection};
use domus::infrastructure::store::StoredPlan;
use sha2::{Digest, Sha256};

fn settings(policy: AudioPolicy) -> OutputSettings {
    OutputSettings {
        format: policy.preferred.format(),
        accepted_formats: Some(policy.formats()),
        audio_policy: Some(policy),
        ..OutputSettings::default()
    }
}

#[test]
fn planning_profiles_require_explicit_acceptance_and_freeze_selection() -> TestResult {
    let mut fixture = Fixture::new()?;
    scan_location(&mut fixture.store, fixture.main)?;
    let defaults = fixture.store.settings()?.ok_or("settings")?;
    assert_eq!(defaults.policy, AudioPolicy::default());
    assert_eq!(defaults.audio_profiles.len(), 8);
    for profile in AudioProfile::ALL {
        let mut request = fixture.request()?;
        request.audio_profile = Some(profile);
        assert_eq!(
            fixture.store.prepare_plan(request).is_ok(),
            defaults.policy.accepted.contains(&profile)
        );
    }
    let policy = AudioPolicy {
        version: AUDIO_POLICY_VERSION,
        accepted: AudioProfile::ALL.to_vec(),
        preferred: AudioProfile::Mp3Cbr192,
    };
    fixture.store.save_settings(Some(1), &settings(policy))?;
    let mut request = fixture.request()?;
    request.settings_version = 2;
    let plan = fixture.store.prepare_plan(request)?;
    assert_eq!(
        plan.plan.audio,
        Some(AudioSelection::new(AudioProfile::Mp3Cbr192))
    );
    let mut legacy_request = fixture.request()?;
    legacy_request.settings_version = 2;
    legacy_request.format = Some(AudioFormat::Mp3);
    assert_eq!(
        fixture
            .store
            .prepare_plan(legacy_request)?
            .plan
            .audio_profile()?,
        AudioProfile::Mp3Cbr320
    );
    fixture
        .store
        .save_settings(Some(2), &OutputSettings::default())?;
    assert_eq!(fixture.store.plan(plan.job_id, plan.version)?, plan);
    Ok(())
}

#[test]
fn planning_profiles_reject_invalid_policies_and_conflicting_legacy_fields() -> TestResult {
    let mut policy = AudioPolicy::default();
    for invalid in [
        AudioPolicy {
            version: 99,
            ..policy.clone()
        },
        AudioPolicy {
            accepted: vec![],
            ..policy.clone()
        },
        AudioPolicy {
            accepted: vec![AudioProfile::Mp3Cbr320; 2],
            ..policy.clone()
        },
        AudioPolicy {
            preferred: AudioProfile::Mp3Vbr,
            ..policy.clone()
        },
    ] {
        assert!(settings(invalid).validate().is_err());
    }
    policy.preferred = AudioProfile::FlacLossless;
    let mut invalid = settings(policy);
    invalid.format = AudioFormat::Mp3;
    assert!(invalid.validate().is_err());
    invalid.format = AudioFormat::Flac;
    invalid.accepted_formats = None;
    assert!(invalid.validate().is_err());
    assert!(serde_json::from_str::<AudioProfile>("\"m4a_aac\"").is_err());
    Ok(())
}

#[test]
fn planning_profiles_legacy_serialization_and_fingerprint_are_unchanged() -> TestResult {
    let mut fixture = Fixture::new()?;
    scan_location(&mut fixture.store, fixture.main)?;
    let request = fixture.request()?;
    let mut plan = fixture.store.prepare_plan(request)?;
    plan.plan.audio = None;
    let old = serde_json::to_string(&plan)?;
    assert!(!old.contains("\"audio\":"));
    assert!(!old.contains("audio_policy"));
    let loaded: StoredPlan = serde_json::from_str(&old)?;
    assert_eq!(loaded.plan.audio_profile()?, AudioProfile::Mp3Cbr320);
    assert_eq!(old, serde_json::to_string(&loaded)?);
    assert_eq!(
        Sha256::digest(old.as_bytes()),
        Sha256::digest(serde_json::to_vec(&loaded)?)
    );
    let mut wrong = loaded;
    wrong.plan.audio = Some(AudioSelection {
        policy_version: 99,
        profile: AudioProfile::Mp3Cbr320,
    });
    assert!(wrong.plan.audio_profile().is_err());
    Ok(())
}
