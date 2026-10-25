use super::*;
use domus::application::{export_playback_album, normalize_album};
use domus::infrastructure::flac::inspect_flac;
use domus::infrastructure::mp3::inspect_mp3_320;
use id3::{Tag, TagLike};
use std::io::Read;

#[test]
fn playback_export_has_exact_frozen_names_tags_jpeg_and_unchanged_encoded_audio() -> TestResult {
    for format in [AudioFormat::Mp3, AudioFormat::Flac] {
        let mut fixture = Fixture::build(format, true)?;
        let staging = fixture.staging.as_ref().ok_or("missing staging")?;
        let run = normalize_album(&mut fixture.store, &fixture.claim, staging, &fixture.cover)?;
        let album = export_playback_album(&fixture.store, &fixture.claim, staging, run)?;
        let outputs = fixture
            .store
            .plan(fixture.job, fixture.version)?
            .plan
            .outputs;
        let mut expected = vec!["folder.jpg".to_owned()];
        for (index, output) in outputs.iter().enumerate() {
            expected.push(output.filename.clone());
            let file = album.join(&output.filename);
            assert!(file.is_file());
            let bytes = std::fs::read(&file)?;
            match format {
                AudioFormat::Mp3 => {
                    let tag = Tag::read_from2(&mut Cursor::new(&bytes))?;
                    assert_eq!(tag.track(), Some(index as u32 + 1));
                    assert_eq!(tag.album(), Some(super::super::ALBUM));
                    assert_eq!(tag.artist(), Some(super::super::ARTIST_TEXT));
                    assert_eq!(
                        tag.pictures().next().ok_or("missing picture")?.data,
                        fixture.cover.bytes()
                    );
                    let before = inspect_mp3_320(&mut Cursor::new(&fixture.source))?;
                    let after = inspect_mp3_320(&mut Cursor::new(&bytes))?;
                    assert_eq!(before.mpeg_sha256, after.mpeg_sha256);
                }
                AudioFormat::Flac => {
                    let tag = metaflac::Tag::read_from(&mut Cursor::new(&bytes))?;
                    assert_eq!(
                        tag.get_vorbis("TRACKNUMBER").ok_or("missing track")?.next(),
                        Some((index + 1).to_string().as_str())
                    );
                    assert_eq!(
                        tag.get_vorbis("ALBUM").ok_or("missing album")?.next(),
                        Some(super::super::ALBUM)
                    );
                    assert_eq!(
                        tag.pictures().next().ok_or("missing picture")?.data,
                        fixture.cover.bytes()
                    );
                    let before = inspect_flac(&mut Cursor::new(&fixture.source))?;
                    let after = inspect_flac(&mut Cursor::new(&bytes))?;
                    assert_eq!(before.audio_sha256, after.audio_sha256);
                }
            }
        }
        let mut actual = std::fs::read_dir(&album)?
            .map(|entry| entry.map(|entry| entry.file_name().to_string_lossy().into_owned()))
            .collect::<std::io::Result<Vec<_>>>()?;
        actual.sort();
        expected.sort();
        assert_eq!(actual, expected);
        assert_eq!(
            std::fs::read(album.join("folder.jpg"))?,
            fixture.cover.bytes()
        );
        let mut expected_path = fixture.scratch.path().join(format!("playback-{run}"));
        for component in outputs[0].relative_folder.split('/') {
            expected_path.push(component);
        }
        assert_eq!(
            std::fs::canonicalize(&album)?,
            std::fs::canonicalize(&expected_path)?
        );
        let complete = fixture
            .scratch
            .path()
            .join(format!("playback-{run}"))
            .join("complete.json");
        let record: serde_json::Value = serde_json::from_slice(&std::fs::read(complete)?)?;
        assert_eq!(record["purpose"], "playback-test");
        assert_eq!(record["files"], 3);
        assert_eq!(record["run_id"], run);
        assert_eq!(
            fixture.store.job_state(fixture.job)?.as_deref(),
            Some("publishing")
        );
        let retained = std::fs::read(album.join(&outputs[0].filename))?;
        assert!(export_playback_album(&fixture.store, &fixture.claim, staging, run).is_err());
        assert_eq!(std::fs::read(album.join(&outputs[0].filename))?, retained);
        assert_eq!(std::fs::read_dir(fixture._library.path())?.count(), 0);
    }
    Ok(())
}

#[test]
fn stale_claim_changed_output_or_changed_observation_stops_export_before_creation() -> TestResult {
    for fault in ["claim", "output", "observation", "cover"] {
        let mut fixture = Fixture::build(AudioFormat::Flac, true)?;
        let staging = fixture.staging.as_ref().ok_or("missing staging")?;
        let run = normalize_album(&mut fixture.store, &fixture.claim, staging, &fixture.cover)?;
        let normalized = fixture.scratch.path().join(format!("normalization-{run}"));
        match fault {
            "claim" => {
                Connection::open(&fixture.database)?
                    .execute("UPDATE jobs SET lease_until=0", [])?;
            }
            "output" => {
                std::fs::write(normalized.join("track-0002.flac"), b"retain damaged output")?;
            }
            "cover" => {
                std::fs::write(normalized.join("folder.jpg"), b"retain damaged cover")?;
            }
            _ => {
                scan_location(&mut fixture.store, fixture.main)?;
            }
        }
        assert!(export_playback_album(&fixture.store, &fixture.claim, staging, run).is_err());
        assert!(
            !fixture
                .scratch
                .path()
                .join(format!("playback-{run}"))
                .exists()
        );
        if fault == "output" {
            assert_eq!(
                std::fs::read(normalized.join("track-0002.flac"))?,
                b"retain damaged output"
            );
        }
        assert_eq!(
            fixture.store.job_state(fixture.job)?.as_deref(),
            Some("publishing")
        );
    }
    Ok(())
}

#[test]
fn partial_existing_export_is_never_overwritten_or_reported_complete() -> TestResult {
    let mut fixture = Fixture::build(AudioFormat::Flac, true)?;
    let staging = fixture.staging.as_ref().ok_or("missing staging")?;
    let run = normalize_album(&mut fixture.store, &fixture.claim, staging, &fixture.cover)?;
    let partial = fixture.scratch.path().join(format!("playback-{run}"));
    std::fs::create_dir(&partial)?;
    std::fs::write(partial.join("sentinel"), b"retain partial export")?;
    assert!(export_playback_album(&fixture.store, &fixture.claim, staging, run).is_err());
    assert!(!partial.join("complete.json").exists());
    let mut bytes = Vec::new();
    std::fs::File::open(partial.join("sentinel"))?.read_to_end(&mut bytes)?;
    assert_eq!(bytes, b"retain partial export");
    assert_eq!(std::fs::read_dir(partial)?.count(), 1);
    Ok(())
}
