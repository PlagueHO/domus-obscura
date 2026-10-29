use domus::application::{add_location, library_jpeg_input, refresh_library_folder, scan_location};
use domus::domain::library::LibraryMatchQuery;
use domus::domain::{LibraryPath, Page};
use domus::infrastructure::store::{Observation, Store};
use id3::TagLike;
use std::io::Write;
use std::path::Path;
use tempfile::tempdir;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn page() -> domus::Result<Page> {
    Page::new(0, 200)
}

fn mp3(path: &Path, bitrate: u8, artist: Option<&str>, album: Option<&str>) -> TestResult {
    let mut tag = id3::Tag::new();
    tag.set_title("Generated track");
    if let Some(artist) = artist {
        tag.set_artist(artist);
    }
    if let Some(album) = album {
        tag.set_album(album);
    }
    tag.set_track(2);
    tag.set_total_tracks(10);
    tag.set_disc(1);
    tag.set_total_discs(2);
    let mut file = std::fs::File::create(path)?;
    tag.write_to(&mut file, id3::Version::Id3v24)?;
    let kbps = match bitrate {
        9 => 128,
        14 => 320,
        _ => 64,
    };
    for _ in 0..3 {
        let mut frame = vec![0; 144000 * kbps / 44100];
        frame[..4].copy_from_slice(&[0xff, 0xfb, bitrate << 4, 0]);
        file.write_all(&frame)?;
    }
    Ok(())
}

fn flac(path: &Path) -> TestResult {
    let mut file = std::fs::File::create(path)?;
    file.write_all(b"fLaC")?;
    file.write_all(&[0, 0, 0, 34])?;
    let mut stream = [0; 34];
    let fields = (44100u64 << 44) | (1 << 41) | (15 << 36) | 44100;
    stream[10..18].copy_from_slice(&fields.to_be_bytes());
    file.write_all(&stream)?;
    let mut comments = vec![0, 0, 0, 0];
    let fields = [
        "ARTIST=Track artist",
        "ALBUMARTIST=Album artist",
        "ALBUM=FLAC album",
        "TITLE=FLAC track",
        "TRACKNUMBER=1",
        "TRACKTOTAL=12",
        "MUSICBRAINZ_ALBUMID=observed-not-proof",
    ];
    comments.extend_from_slice(&(fields.len() as u32).to_le_bytes());
    for field in fields {
        comments.extend_from_slice(&(field.len() as u32).to_le_bytes());
        comments.extend_from_slice(field.as_bytes());
    }
    let length = comments.len() as u32;
    file.write_all(&[
        0x84,
        (length >> 16) as u8,
        (length >> 8) as u8,
        length as u8,
    ])?;
    file.write_all(&comments)?;
    Ok(())
}

#[test]
fn library_tags_counts_locations_and_read_only_files() -> TestResult {
    let state = tempdir()?;
    let main = tempdir()?;
    let backup = tempdir()?;
    for folder in ["original", "other-edition", "flac"] {
        std::fs::create_dir(main.path().join(folder))?;
    }

    mp3(
        &main.path().join("original/one.mp3"),
        9,
        Some("Artist"),
        Some("Album"),
    )?;
    mp3(
        &main.path().join("other-edition/one.mp3"),
        14,
        Some("Artist"),
        Some("Album"),
    )?;
    mp3(
        &backup.path().join("copy.mp3"),
        9,
        Some("Artist"),
        Some("Album"),
    )?;
    flac(&main.path().join("flac/one.flac"))?;
    std::fs::write(
        main.path().join("original/folder.jpg"),
        [0xff, 0xd8, 0xff, 0xd9],
    )?;
    std::fs::write(main.path().join("invalid.mp3"), b"not MPEG")?;
    std::fs::write(main.path().join("future.m4a"), b"unsupported, preserved")?;
    let before = std::fs::read(main.path().join("original/one.mp3"))?;
    let modified = std::fs::metadata(main.path().join("original/one.mp3"))?.modified()?;
    let mut store = Store::open(&state.path().join("state.db"))?;
    let main_id = add_location(&store, main.path(), "main")?;
    let backup_id = add_location(&store, backup.path(), "backup")?;
    scan_location(&mut store, main_id)?;
    scan_location(&mut store, backup_id)?;
    let artists = store.library_artists(main_id, page()?)?;
    assert_eq!(artists.len(), 2);
    assert_eq!(
        artists
            .iter()
            .find(|artist| artist.name == "Artist")
            .ok_or("missing artist")?
            .file_count,
        2
    );
    let albums = store.library_albums(main_id, None, page()?)?;
    assert_eq!(albums.len(), 3);
    let copies = albums
        .iter()
        .filter(|a| a.title == "Album")
        .collect::<Vec<_>>();
    assert_eq!(copies.len(), 2);
    assert_ne!(copies[0].folder, copies[1].folder);
    assert_eq!(store.library_albums(backup_id, None, page()?)?.len(), 1);
    assert!(store.library_album(backup_id, copies[0].id).is_err());
    let files = store.library_album_files(main_id, copies[0].id, page()?)?;
    assert_eq!(files.len(), 1);
    assert!(matches!(
        files[0].metadata.audio.bitrate_kbps,
        Some(128 | 320)
    ));
    assert_eq!(files[0].metadata.tags.track.as_deref(), Some("2/10"));
    let flac_album = albums
        .iter()
        .find(|a| a.title == "FLAC album")
        .ok_or("missing FLAC")?;
    assert_eq!(flac_album.artist, "Album artist");
    let file = store
        .library_album_files(main_id, flac_album.id, page()?)?
        .remove(0);
    assert_eq!(file.metadata.audio.duration_ms, Some(1000));
    assert_eq!(file.metadata.audio.bits_per_sample, Some(16));
    assert_eq!(file.metadata.tags.artists, ["Track artist"]);
    assert_eq!(file.metadata.tags.track.as_deref(), Some("1/12"));
    assert_eq!(
        file.metadata.tags.identifiers["MUSICBRAINZ_ALBUMID"],
        ["observed-not-proof"]
    );
    let root = LibraryPath::parse("")?;
    let root_files = store.browse(main_id, &root, page()?)?;
    for (name, status) in [("invalid.mp3", "unreadable"), ("future.m4a", "unsupported")] {
        let entry = root_files
            .iter()
            .find(|e| e.relative_path == name)
            .ok_or("missing entry")?;
        assert_eq!(
            store.library_file(main_id, entry.id)?.metadata.status,
            status
        );
    }
    let matches = store.library_matches(
        main_id,
        &[
            LibraryMatchQuery {
                artist: " artist ".into(),
                album: Some("album".into()),
            },
            LibraryMatchQuery {
                artist: "Artist!".into(),
                album: Some("Album".into()),
            },
        ],
    )?;
    assert_eq!(matches.matches[0].albums.len(), 2);
    assert!(matches.matches[1].albums.is_empty());
    assert_eq!(matches.coverage.metadata_files, 6);
    assert_eq!(matches.coverage.metadata_issues, 2);
    assert_eq!(matches.coverage.unobserved_files, 0);
    assert_eq!(store.location(main_id)?.coverage, "complete");
    assert_eq!(std::fs::read(main.path().join("original/one.mp3"))?, before);
    assert_eq!(
        std::fs::metadata(main.path().join("original/one.mp3"))?.modified()?,
        modified
    );
    let first = store
        .library_albums(main_id, None, Page::new(0, 1)?)?
        .remove(0);
    let second = store
        .library_albums(main_id, None, Page::new(first.id, 1)?)?
        .remove(0);
    assert!(second.id > first.id);
    Ok(())
}

#[test]
fn library_scan_ignores_prepared_publication_folders() -> TestResult {
    let state = tempdir()?;
    let media = tempdir()?;
    let pending = media.path().join(".domus-pending-7-1");
    std::fs::create_dir(&pending)?;
    mp3(
        &pending.join("one.mp3"),
        9,
        Some("Hidden artist"),
        Some("Incomplete album"),
    )?;
    let visible = media.path().join("visible");
    std::fs::create_dir(&visible)?;
    mp3(
        &visible.join("one.mp3"),
        9,
        Some("Visible artist"),
        Some("Complete album"),
    )?;

    let mut store = Store::open(&state.path().join("state.db"))?;
    let location = add_location(&store, media.path(), "main")?;
    scan_location(&mut store, location)?;

    let root = store.browse(location, &LibraryPath::parse("")?, page()?)?;
    assert!(
        root.iter()
            .all(|entry| entry.relative_path != ".domus-pending-7-1")
    );
    let artists = store.library_artists(location, page()?)?;
    assert_eq!(artists.len(), 1);
    assert_eq!(artists[0].name, "Visible artist");
    let albums = store.library_albums(location, None, page()?)?;
    assert_eq!(albums.len(), 1);
    assert_eq!(albums[0].title, "Complete album");
    Ok(())
}

#[test]
fn library_missing_partial_and_multiple_artist_tags_remain_distinct() -> TestResult {
    let state = tempdir()?;
    let media = tempdir()?;
    mp3(
        &media.path().join("multiple.mp3"),
        9,
        Some("Artist A\0Artist B"),
        Some("Album"),
    )?;
    mp3(&media.path().join("partial.mp3"), 9, None, Some("Album"))?;
    let mut untagged = std::fs::File::create(media.path().join("untagged.mp3"))?;
    for _ in 0..3 {
        let mut frame = vec![0; 417];
        frame[..4].copy_from_slice(&[0xff, 0xfb, 0x90, 0]);
        untagged.write_all(&frame)?;
    }
    drop(untagged);
    let mut store = Store::open(&state.path().join("state.db"))?;
    let location = add_location(&store, media.path(), "main")?;
    scan_location(&mut store, location)?;
    let entries = store.browse(location, &LibraryPath::parse("")?, page()?)?;
    let file = |name| -> domus::Result<_> {
        let entry = entries
            .iter()
            .find(|entry| entry.relative_path == name)
            .ok_or(domus::Error::Internal("missing generated entry"))?;
        store.library_file(location, entry.id)
    };
    assert_eq!(file("untagged.mp3")?.metadata.status, "missing_tags");
    let partial = file("partial.mp3")?;
    assert!(partial.metadata.tags.artists.is_empty());
    assert_eq!(partial.metadata.tags.album.as_deref(), Some("Album"));
    assert_eq!(
        file("multiple.mp3")?.metadata.tags.artists,
        ["Artist A", "Artist B"]
    );
    assert_eq!(
        store.library_artists(location, page()?)?[0].name,
        "Artist A; Artist B"
    );
    assert_eq!(
        store.library_albums(location, None, page()?)?[0].file_count,
        1
    );
    assert_eq!(entries.len(), 3);
    Ok(())
}

#[test]
fn library_folder_refresh_is_direct_scoped_and_never_renews_root_coverage() -> TestResult {
    let state = tempdir()?;
    let main = tempdir()?;
    std::fs::create_dir_all(main.path().join("selected/child"))?;
    std::fs::create_dir(main.path().join("sibling"))?;
    for path in [
        "selected/one.mp3",
        "selected/child/two.mp3",
        "sibling/three.mp3",
    ] {
        mp3(&main.path().join(path), 9, Some("Artist"), Some("Album"))?;
    }
    let database = state.path().join("state.db");
    let mut store = Store::open(&database)?;
    let location = add_location(&store, main.path(), "main")?;
    scan_location(&mut store, location)?;
    let prior = store.location(location)?;
    let descendant = store
        .library_albums(location, None, page()?)?
        .into_iter()
        .find(|a| a.folder == "selected/child")
        .ok_or("missing child")?;
    std::fs::remove_file(main.path().join("selected/one.mp3"))?;
    std::fs::remove_file(main.path().join("selected/child/two.mp3"))?;
    mp3(
        &main.path().join("selected/new.mp3"),
        14,
        Some("New artist"),
        Some("New album"),
    )?;
    refresh_library_folder(&mut store, location, &LibraryPath::parse("selected")?)?;
    assert_eq!(store.location(location)?.observed_at, prior.observed_at);
    assert_eq!(store.location(location)?.coverage, prior.coverage);
    assert_eq!(store.library_album(location, descendant.id)?.file_count, 1);
    let folder = store.browse(location, &LibraryPath::parse("selected")?, page()?)?;
    assert!(
        !folder
            .iter()
            .find(|e| e.relative_path.ends_with("one.mp3"))
            .ok_or("old file")?
            .present
    );
    assert!(
        folder
            .iter()
            .find(|e| e.relative_path.ends_with("new.mp3"))
            .ok_or("new file")?
            .present
    );
    assert_eq!(
        store
            .library_coverage(location, &LibraryPath::parse("selected")?)?
            .folder_state
            .as_deref(),
        Some("complete")
    );
    std::fs::rename(main.path().join("selected"), main.path().join("moved"))?;
    assert!(
        refresh_library_folder(&mut store, location, &LibraryPath::parse("selected")?).is_err()
    );
    assert!(
        store
            .browse(location, &LibraryPath::parse("selected")?, page()?)?
            .iter()
            .any(|e| e.relative_path.ends_with("new.mp3") && e.present)
    );
    assert_eq!(
        store
            .library_coverage(location, &LibraryPath::parse("selected")?)?
            .folder_state
            .as_deref(),
        Some("incomplete")
    );
    assert!(store.jobs(page()?)?.is_empty());
    Ok(())
}

#[test]
fn library_scan_scope_and_claims_are_fenced() -> TestResult {
    let state = tempdir()?;
    let media = tempdir()?;
    std::fs::create_dir(media.path().join("folder"))?;
    mp3(
        &media.path().join("folder/one.mp3"),
        9,
        Some("A"),
        Some("B"),
    )?;
    let database = state.path().join("state.db");
    let mut store = Store::open(&database)?;
    let location = add_location(&store, media.path(), "main")?;
    let folder = LibraryPath::parse("folder")?;
    let old = store.begin_folder_scan(location, &folder)?;
    assert!(store.begin_scan(location).is_err());
    let outside = LibraryPath::parse("outside.mp3")?;
    assert!(
        store
            .record_batch(
                &old,
                &[Observation {
                    path: &outside,
                    kind: "file",
                    size: Some(1),
                    modified_at: None
                }]
            )
            .is_err()
    );
    rusqlite::Connection::open(&database)?.execute("UPDATE scans SET lease_until=0", [])?;
    assert!(store.begin_scan(location).is_err());
    let new = store.begin_folder_scan(location, &folder)?;
    assert_eq!(old.id, new.id);
    assert!(store.record_batch(&old, &[]).is_err());
    rusqlite::Connection::open(&database)?.execute("UPDATE scans SET lease_until=0", [])?;
    refresh_library_folder(&mut store, location, &folder)?;
    assert_eq!(store.library_albums(location, None, page()?)?.len(), 1);
    assert_eq!(store.location(location)?.coverage, "unknown");
    Ok(())
}

#[test]
fn library_failed_metadata_replaces_old_matches_and_previews_recheck_bindings() -> TestResult {
    let state = tempdir()?;
    let media = tempdir()?;
    mp3(
        &media.path().join("one.mp3"),
        9,
        Some("Artist"),
        Some("Album"),
    )?;
    std::fs::write(media.path().join("folder.jpg"), [0xff, 0xd8, 0xff, 0xd9])?;
    let mut store = Store::open(&state.path().join("state.db"))?;
    let location = add_location(&store, media.path(), "main")?;
    scan_location(&mut store, location)?;
    let root = LibraryPath::parse("")?;
    let entries = store.browse(location, &root, page()?)?;
    let asset = entries
        .iter()
        .find(|e| e.relative_path == "folder.jpg")
        .ok_or("missing JPEG")?
        .id;
    assert_eq!(
        library_jpeg_input(&store, location, asset)?,
        [0xff, 0xd8, 0xff, 0xd9]
    );
    std::fs::rename(
        media.path().join("folder.jpg"),
        media.path().join("old.jpg"),
    )?;
    std::fs::write(media.path().join("folder.jpg"), [0xff, 0xd8, 0xff, 0xd9])?;
    assert!(library_jpeg_input(&store, location, asset).is_err());
    std::fs::write(media.path().join("one.mp3"), b"broken replacement")?;
    refresh_library_folder(&mut store, location, &root)?;
    assert!(store.library_artists(location, page()?)?.is_empty());
    assert!(store.library_albums(location, None, page()?)?.is_empty());
    assert!(
        store
            .library_matches(
                location,
                &[LibraryMatchQuery {
                    artist: "Artist".into(),
                    album: Some("Album".into())
                }]
            )?
            .matches[0]
            .albums
            .is_empty()
    );
    assert!(
        library_jpeg_input(
            &store,
            location,
            entries
                .iter()
                .find(|e| e.relative_path == "one.mp3")
                .ok_or("missing MP3")?
                .id
        )
        .is_err()
    );
    Ok(())
}

#[test]
fn library_preview_distinguishes_changes_to_pre_epoch_timestamps() -> TestResult {
    let state = tempdir()?;
    let media = tempdir()?;
    let path = media.path().join("cover.jpg");
    let mut file = std::fs::File::create(&path)?;
    file.write_all(&[0xff, 0xd8, 0xff, 0xd9])?;
    file.set_modified(std::time::UNIX_EPOCH - std::time::Duration::from_secs(10))?;
    drop(file);
    let mut store = Store::open(&state.path().join("state.db"))?;
    let location = add_location(&store, media.path(), "main")?;
    scan_location(&mut store, location)?;
    let entry = store
        .browse(location, &LibraryPath::parse("")?, page()?)?
        .remove(0)
        .id;
    assert_eq!(
        library_jpeg_input(&store, location, entry)?,
        [0xff, 0xd8, 0xff, 0xd9]
    );
    let mut file = std::fs::File::options().write(true).open(&path)?;
    file.write_all(&[0xff, 0xd8, 0x11, 0xd9])?;
    file.set_modified(std::time::UNIX_EPOCH - std::time::Duration::from_secs(20))?;
    drop(file);
    assert!(matches!(
        library_jpeg_input(&store, location, entry),
        Err(domus::Error::Conflict(_))
    ));
    Ok(())
}

#[test]
fn library_previews_and_scoped_refresh_reject_root_and_directory_substitution() -> TestResult {
    let state = tempdir()?;
    let scratch = tempdir()?;
    let outside = tempdir()?;
    let root = scratch.path().join("media");
    let retained = scratch.path().join("retained");
    std::fs::create_dir_all(root.join("selected"))?;
    let jpeg = [0xff, 0xd8, 0xff, 0xd9];
    std::fs::write(root.join("selected").join("cover.jpg"), jpeg)?;
    std::fs::write(outside.path().join("cover.jpg"), jpeg)?;
    let mut store = Store::open(&state.path().join("state.db"))?;
    let location = add_location(&store, &root, "main")?;
    scan_location(&mut store, location)?;
    let folder = LibraryPath::parse("selected")?;
    let asset = store.browse(location, &folder, page()?)?.remove(0).id;
    std::fs::rename(&root, &retained)?;
    std::fs::create_dir_all(root.join("selected"))?;
    std::fs::write(root.join("selected").join("cover.jpg"), jpeg)?;
    assert!(library_jpeg_input(&store, location, asset).is_err());
    assert!(refresh_library_folder(&mut store, location, &folder).is_err());
    assert!(store.library_file(location, asset)?.present);
    std::fs::remove_file(root.join("selected").join("cover.jpg"))?;
    std::fs::remove_dir(root.join("selected"))?;
    std::fs::remove_dir(&root)?;
    std::fs::rename(&retained, &root)?;
    std::fs::rename(root.join("selected"), root.join("old-selected"))?;
    #[cfg(windows)]
    {
        let output = std::process::Command::new("cmd.exe")
            .args(["/d", "/c", "mklink", "/J"])
            .arg(root.join("selected"))
            .arg(outside.path())
            .output()?;
        assert!(
            output.status.success(),
            "fixture junction failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    #[cfg(unix)]
    std::os::unix::fs::symlink(outside.path(), root.join("selected"))?;
    assert!(library_jpeg_input(&store, location, asset).is_err());
    assert!(refresh_library_folder(&mut store, location, &folder).is_err());
    assert!(store.library_file(location, asset)?.present);
    assert_eq!(std::fs::read(outside.path().join("cover.jpg"))?, jpeg);
    #[cfg(windows)]
    std::fs::remove_dir(root.join("selected"))?;
    #[cfg(unix)]
    std::fs::remove_file(root.join("selected"))?;
    Ok(())
}

#[test]
fn library_metadata_limits_and_batch_replay_preserve_counts() -> TestResult {
    let state = tempdir()?;
    let media = tempdir()?;
    for index in 0..130 {
        mp3(
            &media.path().join(format!("{index}.mp3")),
            9,
            Some("Artist"),
            Some("Album"),
        )?;
    }
    std::fs::write(
        media.path().join("large.mp3"),
        [b'I', b'D', b'3', 4, 0, 0, 0x7f, 0x7f, 0x7f, 0x7f],
    )?;
    let mut store = Store::open(&state.path().join("state.db"))?;
    let location = add_location(&store, media.path(), "main")?;
    for _ in 0..2 {
        scan_location(&mut store, location)?;
        assert_eq!(store.library_artists(location, page()?)?[0].file_count, 130);
        assert_eq!(
            store.library_albums(location, None, page()?)?[0].file_count,
            130
        );
        let coverage = store.library_coverage(location, &LibraryPath::parse("")?)?;
        assert_eq!(
            (
                coverage.metadata_files,
                coverage.metadata_issues,
                coverage.unobserved_files
            ),
            (131, 1, 0)
        );
    }
    let limit = store
        .browse(location, &LibraryPath::parse("")?, page()?)?
        .into_iter()
        .find(|e| e.relative_path == "large.mp3")
        .ok_or("missing large")?;
    assert_eq!(
        store.library_file(location, limit.id)?.metadata.status,
        "limit"
    );
    Ok(())
}
