use super::*;
use domus::application::{normalize_album, publish_new_album};
use domus::domain::{TagMode, TagPolicy, TagRule};
use domus::infrastructure::media::source_tags;
use id3::TagLike;

include!("../support/mpeg.rs");

#[test]
fn tag_policy_review_is_exact_durable_and_cannot_be_bypassed() -> TestResult {
    for format in [AudioFormat::Mp3, AudioFormat::Flac] {
        let policy = TagPolicy {
            genre: TagRule::Retain {},
            composer: TagRule::Update {
                value: "  Björk  ".into(),
                mode: TagMode::Automatic,
            },
            ..Default::default()
        };
        let mut fixture = Fixture::build_with_inputs(
            format,
            true,
            true,
            true,
            None,
            None,
            FixtureInputs {
                tag_policy: Some(policy),
                ..Default::default()
            },
        )?;
        let staging = fixture.staging.as_ref().ok_or("staging")?;
        assert!(matches!(
            fixture
                .store
                .begin_normalization(&fixture.claim, &fixture.cover),
            Err(domus::Error::Conflict(_))
        ));
        let paused = normalize_album(&mut fixture.store, &fixture.claim, staging, &fixture.cover);
        assert!(
            matches!(paused, Err(domus::Error::WorkerWaiting)),
            "{paused:?}"
        );
        assert_eq!(
            fixture.store.job_state(fixture.job)?.as_deref(),
            Some("awaiting_input")
        );
        let review = fixture
            .store
            .tag_review(fixture.job, fixture.version, Page::new(0, 200)?)?
            .ok_or("review")?;
        assert_eq!(review.tracks.len(), 3);
        assert!(!review.approved);
        let page = fixture
            .store
            .tag_review(fixture.job, fixture.version, Page::new(1, 1)?)?
            .ok_or("review page")?;
        assert_eq!(page.sha256, review.sha256);
        assert_eq!(page.tracks.len(), 1);
        assert_eq!(page.tracks[0].track_number, 2);
        assert!(
            fixture
                .store
                .tag_review(fixture.job, fixture.version, Page::new(3, 1)?)?
                .ok_or("empty review page")?
                .tracks
                .is_empty()
        );
        assert_eq!(
            review.tracks[0].output.composer.as_deref(),
            Some("  Björk  ")
        );
        assert_eq!(
            review.tracks[0].output.genre.as_deref(),
            (format == AudioFormat::Mp3).then_some("remove old genre")
        );
        assert!(
            review
                .tracks
                .iter()
                .all(|track| track.output.comment.is_none())
        );
        assert!(
            fixture
                .store
                .answer(review.question_id, fixture.version, "Approve exact tags")
                .is_err()
        );
        assert!(
            fixture
                .store
                .resume_tag_review(fixture.job, fixture.version, &review.sha256)
                .is_err()
        );
        assert!(
            fixture
                .store
                .approve_tag_review(fixture.job, fixture.version, &"0".repeat(64))
                .is_err()
        );
        fixture
            .store
            .approve_tag_review(fixture.job, fixture.version, &review.sha256)?;
        assert!(fixture.store.claim(fixture.job, "bypass", 300)?.is_none());
        fixture
            .store
            .save_settings(Some(1), &OutputSettings::default())?;
        fixture
            .store
            .resume_tag_review(fixture.job, fixture.version, &review.sha256)?;
        assert!(
            fixture
                .store
                .resume_tag_review(fixture.job, fixture.version, &review.sha256)
                .is_err()
        );
        let mut store = fixture.open_store()?;
        let claim = store
            .claim_approved_plan(fixture.job, fixture.version, "resumed-worker", 300)?
            .ok_or("resumed claim")?
            .claim;
        let path = fixture
            .scratch
            .path()
            .join(format!("transfer-{}.input", review.tracks[0].transfer_id));
        let mut changed = fixture.source.clone();
        changed[0] ^= 1;
        std::fs::write(&path, changed)?;
        assert!(normalize_album(&mut store, &claim, staging, &fixture.cover).is_err());
        std::fs::write(&path, &fixture.source)?;
        let run = normalize_album(&mut store, &claim, staging, &fixture.cover)?;
        publish_new_album(&mut store, &claim, staging, run)?;
        let plan = store.plan(fixture.job, fixture.version)?;
        let folder = PathBuf::from(&plan.plan.destinations[0].root).join(
            plan.plan.outputs[0]
                .relative_folder
                .replace('/', std::path::MAIN_SEPARATOR_STR),
        );
        for output in &plan.plan.outputs {
            let bytes = std::fs::read(folder.join(&output.filename))?;
            if format == AudioFormat::Mp3 {
                let tag = id3::Tag::read_from2(&mut Cursor::new(&bytes))?;
                assert_eq!(tag.genre(), Some("remove old genre"));
                assert_eq!(
                    tag.get("TCOM").and_then(|frame| frame.content().text()),
                    Some("  Björk  ")
                );
                assert_eq!(tag.comments().count(), 0);
                assert!(
                    !tag.extended_texts()
                        .any(|frame| frame.description.starts_with("REPLAYGAIN"))
                );
                let before =
                    domus::infrastructure::mp3::inspect_mp3_320(&mut Cursor::new(&fixture.source))?;
                let after = domus::infrastructure::mp3::inspect_mp3_320(&mut Cursor::new(&bytes))?;
                assert_eq!(before.mpeg_sha256, after.mpeg_sha256);
            } else {
                let tag = metaflac::Tag::read_from(&mut Cursor::new(&bytes))?;
                assert_eq!(
                    tag.get_vorbis("COMPOSER")
                        .ok_or("composer")?
                        .collect::<Vec<_>>(),
                    ["  Björk  "]
                );
                assert!(tag.get_vorbis("COMMENT").is_none());
                assert!(tag.get_vorbis("GENRE").is_none());
                let before =
                    domus::infrastructure::flac::inspect_flac(&mut Cursor::new(&fixture.source))?;
                let after = domus::infrastructure::flac::inspect_flac(&mut Cursor::new(&bytes))?;
                assert_eq!(before.audio_sha256, after.audio_sha256);
            }
        }
    }
    Ok(())
}

#[test]
fn tag_policy_automatic_updates_and_removals_need_no_extra_approval() -> TestResult {
    for profile in domus::domain::AudioProfile::ALL {
        let format = profile.format();
        let source = if format == AudioFormat::Flac {
            flac()
        } else if let Some(bitrate) = profile.bitrate() {
            frames(&[bitrate; 3], false, false, 0, false)
        } else {
            frames(&[32, 128, 320], false, false, 0, false)
        };
        let policy = TagPolicy {
            genre: TagRule::Update {
                value: "New genre".into(),
                mode: TagMode::Automatic,
            },
            comment: TagRule::Update {
                value: "  Exact comment  ".into(),
                mode: TagMode::Automatic,
            },
            copyright: TagRule::Update {
                value: "2026 Exact owner".into(),
                mode: TagMode::Automatic,
            },
            ..Default::default()
        };
        let mut fixture = Fixture::build_with_inputs(
            format,
            true,
            true,
            true,
            None,
            None,
            FixtureInputs {
                profile: Some(profile),
                source: Some(source),
                tag_policy: Some(policy),
                ..Default::default()
            },
        )?;
        let staging = fixture.staging.as_ref().ok_or("staging")?;
        let run = normalize_album(&mut fixture.store, &fixture.claim, staging, &fixture.cover)?;
        assert!(
            fixture
                .store
                .tag_review(fixture.job, fixture.version, Page::new(0, 200)?)?
                .is_none()
        );
        assert_eq!(
            fixture.store.job_state(fixture.job)?.as_deref(),
            Some("publishing")
        );
        publish_new_album(&mut fixture.store, &fixture.claim, staging, run)?;
        let plan = fixture.store.plan(fixture.job, fixture.version)?;
        let folder = PathBuf::from(&plan.plan.destinations[0].root).join(
            plan.plan.outputs[0]
                .relative_folder
                .replace('/', std::path::MAIN_SEPARATOR_STR),
        );
        for output in &plan.plan.outputs {
            let bytes = std::fs::read(folder.join(&output.filename))?;
            if format == AudioFormat::Mp3 {
                let before = domus::infrastructure::mp3::inspect_mp3(
                    &mut Cursor::new(&fixture.source),
                    profile,
                )?;
                let after =
                    domus::infrastructure::mp3::inspect_mp3(&mut Cursor::new(&bytes), profile)?;
                assert_eq!(
                    fixture.source
                        [before.mpeg_range.start as usize..before.mpeg_range.end as usize],
                    bytes[after.mpeg_range.start as usize..after.mpeg_range.end as usize]
                );
                let tag = id3::Tag::read_from2(&mut Cursor::new(&bytes))?;
                assert_eq!(
                    tag.get("TCON").and_then(|frame| frame.content().text()),
                    Some("New genre")
                );
                let comments = tag.comments().collect::<Vec<_>>();
                assert_eq!(comments.len(), 1);
                assert_eq!(comments[0].text, "  Exact comment  ");
                assert_eq!(comments[0].lang, "eng");
                assert!(comments[0].description.is_empty());
                assert_eq!(
                    tag.get("TCOP").and_then(|frame| frame.content().text()),
                    Some("2026 Exact owner")
                );
                assert!(tag.get("TCOM").is_none());
            } else {
                let before =
                    domus::infrastructure::flac::inspect_flac(&mut Cursor::new(&fixture.source))?;
                let after = domus::infrastructure::flac::inspect_flac(&mut Cursor::new(&bytes))?;
                assert_eq!(
                    fixture.source[before.audio_range.start as usize..],
                    bytes[after.audio_range.start as usize..]
                );
                let tag = metaflac::Tag::read_from(&mut Cursor::new(&bytes))?;
                assert_eq!(
                    tag.get_vorbis("GENRE").ok_or("genre")?.collect::<Vec<_>>(),
                    ["New genre"]
                );
                assert_eq!(
                    tag.get_vorbis("COMMENT")
                        .ok_or("comment")?
                        .collect::<Vec<_>>(),
                    ["  Exact comment  "]
                );
                assert_eq!(
                    tag.get_vorbis("COPYRIGHT")
                        .ok_or("copyright")?
                        .collect::<Vec<_>>(),
                    ["2026 Exact owner"]
                );
                assert!(tag.get_vorbis("COMPOSER").is_none());
            }
        }
    }
    Ok(())
}

#[test]
fn tag_policy_source_extraction_rejects_duplicate_and_invalid_values_before_collapse() -> TestResult
{
    let policy = TagPolicy {
        genre: TagRule::Retain {},
        ..Default::default()
    };
    for values in [vec!["One", "Two"], vec!["Same", "Same"], vec!["bad\ntext"]] {
        let mut body = Vec::new();
        for value in values {
            body.extend_from_slice(b"TCON");
            let size = value.len() + 1;
            body.extend_from_slice(&[0, 0, (size >> 7) as u8, (size & 127) as u8, 0, 0, 3]);
            body.extend_from_slice(value.as_bytes());
        }
        let size = body.len();
        let mut bytes = b"ID3\x04\x00\x00".to_vec();
        bytes.extend_from_slice(&[0, 0, (size >> 7) as u8, (size & 127) as u8]);
        bytes.extend(body);
        assert!(source_tags(&mut Cursor::new(bytes), AudioFormat::Mp3, &policy).is_err());
    }
    let mut comments = metaflac::block::VorbisComment::new();
    comments
        .comments
        .insert("GENRE".into(), vec!["One".into(), "Two".into()]);
    let body = comments.to_bytes();
    let mut bytes = b"fLaC".to_vec();
    bytes.push(0x84);
    bytes.extend_from_slice(&(body.len() as u32).to_be_bytes()[1..]);
    bytes.extend(body);
    assert!(source_tags(&mut Cursor::new(bytes), AudioFormat::Flac, &policy).is_err());
    Ok(())
}

fn source_tag_mp3(version: u8, frames: &[(&[u8; 4], &[u8])]) -> Vec<u8> {
    fn size(value: usize) -> [u8; 4] {
        [
            ((value >> 21) & 127) as u8,
            ((value >> 14) & 127) as u8,
            ((value >> 7) & 127) as u8,
            (value & 127) as u8,
        ]
    }
    let mut body = Vec::new();
    for (id, value) in frames {
        body.extend_from_slice(*id);
        body.extend_from_slice(&if version == 4 {
            size(value.len())
        } else {
            (value.len() as u32).to_be_bytes()
        });
        body.extend_from_slice(&[0, 0]);
        body.extend_from_slice(value);
    }
    let mut result = vec![b'I', b'D', b'3', version, 0, 0];
    result.extend_from_slice(&size(body.len()));
    result.extend(body);
    result
}

#[test]
fn tag_policy_retained_comments_reach_output_with_the_reviewed_language() -> TestResult {
    for format in [AudioFormat::Mp3, AudioFormat::Flac] {
        let (source, expected) = if format == AudioFormat::Mp3 {
            let original = mp3()?;
            let audio = domus::infrastructure::mp3::inspect_mp3_320(&mut Cursor::new(&original))?;
            let mut source =
                source_tag_mp3(4, &[(b"COMM", b"\x03fra\x00  Exact retained comment  ")]);
            source.extend_from_slice(
                &original[audio.mpeg_range.start as usize..audio.mpeg_range.end as usize],
            );
            (source, "  Exact retained comment  ")
        } else {
            (flac(), "remove old comments")
        };
        let mut fixture = Fixture::build_with_inputs(
            format,
            true,
            true,
            true,
            None,
            None,
            FixtureInputs {
                source: Some(source),
                tag_policy: Some(TagPolicy {
                    comment: TagRule::Retain {},
                    ..Default::default()
                }),
                ..Default::default()
            },
        )?;
        let staging = fixture.staging.as_ref().ok_or("staging")?;
        assert!(matches!(
            normalize_album(&mut fixture.store, &fixture.claim, staging, &fixture.cover),
            Err(domus::Error::WorkerWaiting)
        ));
        let review = fixture
            .store
            .tag_review(fixture.job, fixture.version, Page::new(0, 200)?)?
            .ok_or("review")?;
        assert_eq!(review.tracks[0].source.comment.as_deref(), Some(expected));
        assert_eq!(review.tracks[0].output.comment.as_deref(), Some(expected));
        fixture
            .store
            .approve_tag_review(fixture.job, fixture.version, &review.sha256)?;
        fixture
            .store
            .resume_tag_review(fixture.job, fixture.version, &review.sha256)?;
        let claim = fixture
            .store
            .claim_approved_plan(fixture.job, fixture.version, "comment-worker", 300)?
            .ok_or("claim")?
            .claim;
        let run = normalize_album(&mut fixture.store, &claim, staging, &fixture.cover)?;
        publish_new_album(&mut fixture.store, &claim, staging, run)?;
        let plan = fixture.store.plan(fixture.job, fixture.version)?;
        let output = &plan.plan.outputs[0];
        let path = PathBuf::from(&plan.plan.destinations[0].root)
            .join(
                output
                    .relative_folder
                    .replace('/', std::path::MAIN_SEPARATOR_STR),
            )
            .join(&output.filename);
        let bytes = std::fs::read(path)?;
        if format == AudioFormat::Mp3 {
            let tag = id3::Tag::read_from2(&mut Cursor::new(bytes))?;
            let comments = tag.comments().collect::<Vec<_>>();
            assert_eq!(comments.len(), 1);
            assert_eq!(comments[0].text, expected);
            assert_eq!(comments[0].lang, "fra");
            assert!(comments[0].description.is_empty());
        } else {
            let tag = metaflac::Tag::read_from(&mut Cursor::new(bytes))?;
            assert_eq!(
                tag.get_vorbis("COMMENT")
                    .ok_or("comment")?
                    .collect::<Vec<_>>(),
                [expected]
            );
        }
    }
    Ok(())
}

#[test]
fn tag_policy_comment_retention_preserves_exact_text_and_language() -> TestResult {
    let policy = TagPolicy {
        comment: TagRule::Retain {},
        ..Default::default()
    };
    let utf8 = [b"\x03fra\x00".as_slice(), "  Björk  ".as_bytes()].concat();
    let utf16 = b"\x01deu\xff\xfe\x00\x00\xff\xfeA\x00 \x00";
    for (version, payload, expected, language) in [
        (4, utf8.as_slice(), "  Björk  ", "fra"),
        (3, utf16.as_slice(), "A ", "deu"),
    ] {
        let bytes = source_tag_mp3(version, &[(b"COMM", payload)]);
        let source = source_tags(&mut Cursor::new(bytes), AudioFormat::Mp3, &policy)?;
        assert_eq!(source.comment.as_deref(), Some(expected));
        assert_eq!(source.comment_language.as_deref(), Some(language));
        assert_eq!(policy.resolve(&source)?, source);
    }
    for payload in [
        b"\x03engdescription\x00text".as_slice(),
        b"\x03ENG\x00text",
        b"\x03eng\x00\xff",
        b"\x01eng\x00\x00A\x00",
        b"\x03eng\x00one\x00two",
    ] {
        let bytes = source_tag_mp3(4, &[(b"COMM", payload)]);
        assert!(source_tags(&mut Cursor::new(bytes), AudioFormat::Mp3, &policy).is_err());
    }
    let bytes = source_tag_mp3(4, &[(b"COMM", &utf8), (b"COMM", &utf8)]);
    assert!(source_tags(&mut Cursor::new(bytes), AudioFormat::Mp3, &policy).is_err());
    Ok(())
}

#[test]
fn tag_policy_review_update_and_remove_inspect_source_but_automatic_rules_do_not() -> TestResult {
    for rule in [
        TagRule::Update {
            value: "Replacement".into(),
            mode: TagMode::Review,
        },
        TagRule::Remove {
            mode: TagMode::Review,
        },
    ] {
        let policy = TagPolicy {
            genre: rule,
            ..Default::default()
        };
        let source = source_tags(
            &mut Cursor::new(source_tag_mp3(4, &[(b"TCON", b"\x03(17)/Rock")])),
            AudioFormat::Mp3,
            &policy,
        )?;
        assert_eq!(source.genre.as_deref(), Some("(17)/Rock"));
        let output = policy.resolve(&source)?;
        assert_eq!(
            output.genre.is_some(),
            matches!(policy.genre, TagRule::Update { .. })
        );
        assert!(
            source_tags(
                &mut Cursor::new(source_tag_mp3(4, &[(b"TCON", b"\x03\xff")])),
                AudioFormat::Mp3,
                &policy,
            )
            .is_err()
        );
    }
    assert_eq!(
        source_tags(
            &mut Cursor::new(Vec::<u8>::new()),
            AudioFormat::Mp3,
            &TagPolicy::default()
        )?,
        Default::default()
    );
    Ok(())
}
