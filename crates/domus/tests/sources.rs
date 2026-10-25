use domus::domain::sources::*;
use domus::domain::{AudioFormat, EditionSelection, PlannedTrack};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn edition(titles: &[&str]) -> EditionSelection {
    EditionSelection {
        release_id: "b1234567-1234-4321-9876-123456789abc".into(),
        album_artist: "Artist".into(),
        album: "Album".into(),
        bucket: None,
        tracks: titles
            .iter()
            .map(|title| PlannedTrack {
                artist: "Artist".into(),
                title: (*title).into(),
                disc: Some(1),
                disc_track: None,
            })
            .collect(),
    }
}

fn listing(names: &[&str]) -> SourceFolder {
    SourceFolder {
        peer: "peer".into(),
        folder: "@@share\\Artist\\Album".into(),
        directories: 1,
        files: names
            .iter()
            .map(|name| SourceFileMetadata {
                remote_path: format!("@@share\\Artist\\Album\\{name}"),
                expected_bytes: 1000,
                extension: String::new(),
                bitrate_kbps: Some(320),
                duration_seconds: Some(30),
                vbr: Some(false),
                sample_rate: Some(44100),
                bit_depth: Some(16),
            })
            .collect(),
    }
}

#[test]
fn source_exact_names_and_numbers_map_in_release_order_without_approval_or_encoding_claims()
-> TestResult {
    for (format, names) in [
        (
            AudioFormat::Mp3,
            vec![
                "02 - Artist - Two.MP3",
                "01 - One.mp3",
                "folder.jpg",
                "booklet.pdf",
            ],
        ),
        (
            AudioFormat::Flac,
            vec!["Artist - Two.flac", "one.flac", "folder.jpg"],
        ),
    ] {
        let report = assess_source(&edition(&["One", "Two"]), format, &listing(&names))?;
        assert!(report.complete_mapping);
        assert!(report.advertised_quality_matches);
        assert_eq!(report.tracks.len(), 2);
        assert!(report.problems.is_empty());
        assert!(report.tracks[0].remote_path.to_lowercase().contains("one."));
        assert_eq!(report.tracks[0].track_number, 1);
        assert_eq!(report.tracks[1].track_number, 2);
        assert!(!report.edition_verified);
        assert!(!report.audio_verified);
        assert!(report.requires_user_review);
    }
    Ok(())
}

#[test]
fn source_duplicate_titles_need_unambiguous_numbering_and_never_reuse_one_file() -> TestResult {
    let release = edition(&["One", "One"]);
    for names in [
        vec!["One.flac"],
        vec!["One.flac", "Artist - One.flac"],
        vec!["01 - One.flac", "01 - Artist - One.flac"],
    ] {
        let report = assess_source(&release, AudioFormat::Flac, &listing(&names))?;
        assert!(!report.complete_mapping);
        assert!(
            report
                .problems
                .iter()
                .any(|problem| problem.issue == SourceMappingIssue::AmbiguousTrack)
        );
    }
    let report = assess_source(
        &release,
        AudioFormat::Flac,
        &listing(&["01 - One.flac", "02 - One.flac"]),
    )?;
    assert!(report.complete_mapping);
    Ok(())
}

#[test]
fn source_missing_extra_mixed_unknown_and_changed_title_files_block_complete_mapping() -> TestResult
{
    for names in [
        vec!["01 - One.mp3"],
        vec!["01 - One.mp3", "02 - Two.mp3", "03 - Bonus.mp3"],
        vec!["01 - One.mp3", "02 - Two.flac"],
        vec!["01 - One.mp3", "02 - Two.mp3", "unknown.bin"],
        vec!["01 - One (Live).mp3", "02 - Two.mp3"],
        vec!["02 - One.mp3", "01 - Two.mp3"],
        vec!["01 - One.mp3", "02 - Two.mp3", "another.wav"],
    ] {
        let report = assess_source(
            &edition(&["One", "Two"]),
            AudioFormat::Mp3,
            &listing(&names),
        )?;
        assert!(!report.complete_mapping, "{names:?}");
        assert!(!report.advertised_quality_matches);
        assert!(!report.problems.is_empty());
    }
    Ok(())
}

#[test]
fn source_advertised_mp3_quality_is_separate_from_name_mapping_and_real_validation() -> TestResult {
    for (bitrate, vbr, issue) in [
        (Some(320), Some(false), None),
        (
            Some(320),
            None,
            Some(SourceMappingIssue::AdvertisedQualityUnknown),
        ),
        (
            None,
            Some(false),
            Some(SourceMappingIssue::AdvertisedQualityUnknown),
        ),
        (
            Some(256),
            Some(false),
            Some(SourceMappingIssue::AdvertisedQualityMismatch),
        ),
        (
            Some(320),
            Some(true),
            Some(SourceMappingIssue::AdvertisedQualityMismatch),
        ),
    ] {
        let mut folder = listing(&["One.mp3"]);
        folder.files[0].bitrate_kbps = bitrate;
        folder.files[0].vbr = vbr;
        let report = assess_source(&edition(&["One"]), AudioFormat::Mp3, &folder)?;
        assert!(report.complete_mapping);
        assert_eq!(report.advertised_quality_matches, issue.is_none());
        assert_eq!(report.problems.first().map(|problem| problem.issue), issue);
        assert!(!report.audio_verified);
    }
    Ok(())
}

#[test]
fn source_identifiers_and_bounds_reject_escapes_duplicates_untrusted_attributes_and_empty_release()
-> TestResult {
    for path in ["", "\\root", "a\\..\\b", "a\\b\\", "a//b", "a\nb"] {
        assert!(remote_identifier(path).is_err(), "{path:?}");
    }
    assert_eq!(
        remote_identifier("@@share/Artist/Album")?,
        "@@share\\Artist\\Album"
    );
    let release = edition(&["One"]);
    for fault in [
        "duplicate",
        "escape",
        "bytes",
        "extension",
        "many",
        "peer",
        "empty_release",
    ] {
        let mut folder = listing(&["One.flac"]);
        let mut release = release.clone();
        match fault {
            "duplicate" => {
                let mut duplicate = folder.files[0].clone();
                duplicate.remote_path = duplicate.remote_path.to_uppercase();
                folder.files.push(duplicate);
            }
            "escape" => folder.files[0].remote_path = "@@other\\One.flac".into(),
            "bytes" => folder.files[0].expected_bytes = 0,
            "extension" => folder.files[0].extension = "flac\n".into(),
            "many" => folder.directories = 65,
            "peer" => folder.peer = "bad\npeer".into(),
            "empty_release" => release.tracks.clear(),
            _ => unreachable!(),
        }
        assert!(
            assess_source(&release, AudioFormat::Flac, &folder).is_err(),
            "{fault}"
        );
    }
    let mut folder = listing(&["One.flac"]);
    folder.files[0].extension = "mp3".into();
    let report = assess_source(&release, AudioFormat::Flac, &folder)?;
    assert!(!report.complete_mapping);
    Ok(())
}

#[test]
fn source_mapping_supports_the_exact_thousand_track_limit_without_cross_peer_assembly() -> TestResult
{
    let titles = (1..=1000)
        .map(|number| format!("Track {number}"))
        .collect::<Vec<_>>();
    let names = (1..=1000)
        .rev()
        .map(|number| format!("{number:04} - Track {number}.flac"))
        .collect::<Vec<_>>();
    let mut release = edition(&titles.iter().map(String::as_str).collect::<Vec<_>>());
    let mut folder = listing(&names.iter().map(String::as_str).collect::<Vec<_>>());
    let report = assess_source(&release, AudioFormat::Flac, &folder)?;
    assert!(report.complete_mapping);
    assert_eq!(report.tracks.len(), 1000);
    assert_eq!(report.tracks[999].track_number, 1000);
    assert_eq!(report.peer, "peer");
    release.tracks.push(release.tracks[0].clone());
    assert!(assess_source(&release, AudioFormat::Flac, &folder).is_err());
    release.tracks.pop();
    folder.files.push(folder.files[0].clone());
    assert!(assess_source(&release, AudioFormat::Flac, &folder).is_err());
    Ok(())
}
