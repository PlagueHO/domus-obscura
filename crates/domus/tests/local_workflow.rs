use domus::application::{add_location, scan_location};
use domus::domain::{JobState, LibraryPath, Page};
use domus::infrastructure::store::Store;
use std::fs;
use tempfile::tempdir;

#[test]
fn indexing_is_read_only_and_location_specific() -> Result<(), Box<dyn std::error::Error>> {
    let data = tempdir()?;
    let media = tempdir()?;
    let backup = tempdir()?;
    fs::write(media.path().join("track.mp3"), b"unchanged original bytes")?;
    fs::write(media.path().join("folder.jpg"), b"observed non-audio asset")?;
    fs::write(backup.path().join("track.mp3"), b"backup observation")?;
    let mut store = Store::open(&data.path().join("state.db"))?;
    let main = add_location(&store, media.path(), "main")?;
    let backup = add_location(&store, backup.path(), "backup")?;
    scan_location(&mut store, main)?;
    scan_location(&mut store, backup)?;
    let first = store.browse(main, &LibraryPath::parse("")?, Page::new(0, 1)?)?;
    assert_eq!(first.len(), 1);
    let second = store.browse(main, &LibraryPath::parse("")?, Page::new(first[0].id, 1)?)?;
    assert_eq!(second.len(), 1);
    assert_ne!(first[0].relative_path, second[0].relative_path);
    assert_eq!(
        store
            .browse(backup, &LibraryPath::parse("")?, Page::new(0, 200)?)?
            .len(),
        1
    );
    assert_eq!(
        fs::read(media.path().join("track.mp3"))?,
        b"unchanged original bytes"
    );
    assert_eq!(store.location(main)?.coverage, "complete");
    Ok(())
}

#[test]
fn incomplete_scan_does_not_erase_cached_presence() -> Result<(), Box<dyn std::error::Error>> {
    let data = tempdir()?;
    let media = tempdir()?;
    fs::write(media.path().join("album.flac"), b"original")?;
    let mut store = Store::open(&data.path().join("state.db"))?;
    let location = add_location(&store, media.path(), "main")?;
    scan_location(&mut store, location)?;
    media.close()?;
    assert!(scan_location(&mut store, location).is_err());
    let entries = store.browse(location, &LibraryPath::parse("")?, Page::new(0, 200)?)?;
    assert_eq!(entries.len(), 1);
    assert!(entries[0].present);
    assert_eq!(store.location(location)?.coverage, "incomplete");
    assert!(store.location(location)?.last_error.is_some());
    Ok(())
}

#[test]
fn job_claims_and_transitions_survive_reopen() -> Result<(), Box<dyn std::error::Error>> {
    let data = tempdir()?;
    let path = data.path().join("state.db");
    let mut store = Store::open(&path)?;
    let job = store.enqueue(1)?;
    let claim = store
        .claim(job, "first-worker", 60)?
        .ok_or("missing job claim")?;
    let mut second = Store::open(&path)?;
    assert!(second.claim(job, "second-worker", 60)?.is_none());
    assert!(
        store
            .advance(&claim, JobState::Queued, JobState::Searching)
            .is_err()
    );
    second.advance(&claim, JobState::Queued, JobState::Cancelled)?;
    drop(store);
    drop(second);
    assert_eq!(
        Store::open(&path)?.job_state(job)?.as_deref(),
        Some("cancelled")
    );
    Ok(())
}

#[test]
fn state_and_library_locations_cannot_overlap() -> Result<(), Box<dyn std::error::Error>> {
    let directory = tempdir()?;
    let library = directory.path().join("library");
    fs::create_dir(&library)?;
    let store = Store::open(&directory.path().join("state.db"))?;
    assert!(add_location(&store, directory.path(), "main").is_err());
    add_location(&store, &library, "main")?;
    let child = library.join("nested");
    fs::create_dir(&child)?;
    assert!(add_location(&store, &child, "backup").is_err());
    Ok(())
}

#[test]
fn interrupted_scan_resumes_its_directory_frontier() -> Result<(), Box<dyn std::error::Error>> {
    use domus::infrastructure::store::Observation;
    let data = tempdir()?;
    let media = tempdir()?;
    let database = data.path().join("state.db");
    let mut store = Store::open(&database)?;
    let location = add_location(&store, media.path(), "main")?;
    let first = store.begin_scan(location)?;
    let folder = LibraryPath::parse("ALBUM")?;
    store.record_batch(
        &first,
        &[Observation {
            path: &folder,
            kind: "directory",
            size: None,
            modified_at: None,
        }],
    )?;
    store.complete_directory(&first, &LibraryPath::parse("")?)?;
    assert!(store.begin_scan(location).is_err());
    rusqlite::Connection::open(&database)?.execute("UPDATE scans SET lease_until = 0", [])?;
    let resumed = store.begin_scan(location)?;
    assert_eq!(first.id, resumed.id);
    assert!(store.record_batch(&first, &[]).is_err());
    assert_eq!(
        store
            .next_directory(&resumed)?
            .ok_or("missing frontier")?
            .as_str(),
        "ALBUM"
    );
    store.complete_directory(&resumed, &folder)?;
    assert!(store.finish_scan(&resumed)?);
    Ok(())
}

#[test]
fn linked_directories_are_visible_but_never_traversed() -> Result<(), Box<dyn std::error::Error>> {
    let data = tempdir()?;
    let media = tempdir()?;
    let outside = tempdir()?;
    fs::write(
        outside.path().join("secret.txt"),
        b"not part of the library",
    )?;
    let link = media.path().join("linked");
    link_directory(outside.path(), &link)?;
    let mut store = Store::open(&data.path().join("state.db"))?;
    let location = add_location(&store, media.path(), "main")?;
    scan_location(&mut store, location)?;
    let entries = store.browse(location, &LibraryPath::parse("")?, Page::new(0, 200)?)?;
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].kind, "unsafe_link");
    assert!(
        store
            .browse(location, &LibraryPath::parse("linked")?, Page::new(0, 200)?)?
            .is_empty()
    );
    assert_eq!(
        fs::read(outside.path().join("secret.txt"))?,
        b"not part of the library"
    );
    Ok(())
}

fn link_directory(
    target: &std::path::Path,
    link: &std::path::Path,
) -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(windows)]
    {
        let status = std::process::Command::new("cmd.exe")
            .args(["/d", "/c", "mklink", "/J"])
            .arg(link)
            .arg(target)
            .output()?;
        assert!(
            status.status.success(),
            "junction creation failed: {}",
            String::from_utf8_lossy(&status.stderr)
        );
    }

    #[cfg(unix)]
    std::os::unix::fs::symlink(target, link)?;
    Ok(())
}

#[test]
fn replaced_scan_frontier_preserves_cached_presence_and_records_failure()
-> Result<(), Box<dyn std::error::Error>> {
    use domus::infrastructure::store::Observation;
    let data = tempdir()?;
    let media = tempdir()?;
    let outside = tempdir()?;
    let database = data.path().join("state.db");
    let album = media.path().join("ALBUM");
    fs::create_dir(&album)?;
    fs::write(album.join("original.mp3"), b"original")?;
    fs::write(outside.path().join("sentinel"), b"outside")?;
    let mut store = Store::open(&database)?;
    let location = add_location(&store, media.path(), "main")?;
    scan_location(&mut store, location)?;
    let claim = store.begin_scan(location)?;
    let folder = LibraryPath::parse("ALBUM")?;
    store.record_batch(
        &claim,
        &[Observation {
            path: &folder,
            kind: "directory",
            size: None,
            modified_at: None,
        }],
    )?;
    store.complete_directory(&claim, &LibraryPath::parse("")?)?;
    fs::rename(&album, media.path().join("moved"))?;
    link_directory(outside.path(), &album)?;
    rusqlite::Connection::open(&database)?.execute("UPDATE scans SET lease_until = 0", [])?;
    assert!(scan_location(&mut store, location).is_err());
    let entries = store.browse(location, &folder, Page::new(0, 200)?)?;
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].relative_path, "ALBUM/original.mp3");
    assert!(entries[0].present);
    let state = store.location(location)?;
    assert_eq!(state.coverage, "incomplete");
    assert!(state.last_error.is_some());
    assert_eq!(fs::read(outside.path().join("sentinel"))?, b"outside");
    assert_eq!(
        fs::read(media.path().join("moved").join("original.mp3"))?,
        b"original"
    );
    Ok(())
}

#[test]
fn physical_scan_crosses_batch_boundaries_without_changing_files()
-> Result<(), Box<dyn std::error::Error>> {
    let data = tempdir()?;
    let media = tempdir()?;
    for album in 0..192 {
        let directory = media.path().join(format!("ALBUM-{album:03}"));
        fs::create_dir(&directory)?;
        for track in 0..8 {
            fs::write(
                directory.join(format!("track-{track:02}.mp3")),
                format!("original album {album}, track {track}"),
            )?;
        }
    }
    let mut store = Store::open(&data.path().join("state.db"))?;
    let location = add_location(&store, media.path(), "main")?;
    let started = std::time::Instant::now();
    scan_location(&mut store, location)?;
    let albums = store.browse(location, &LibraryPath::parse("")?, Page::new(0, 200)?)?;
    assert_eq!(albums.len(), 192);
    for album in albums {
        let files = store.browse(
            location,
            &LibraryPath::parse(&album.relative_path)?,
            Page::new(0, 200)?,
        )?;
        assert_eq!(files.len(), 8);
        assert!(files.iter().all(|file| file.kind == "file" && file.present));
    }
    for album in 0..192 {
        for track in 0..8 {
            assert_eq!(
                fs::read(
                    media
                        .path()
                        .join(format!("ALBUM-{album:03}"))
                        .join(format!("track-{track:02}.mp3"))
                )?,
                format!("original album {album}, track {track}").as_bytes()
            );
        }
    }
    assert_eq!(store.location(location)?.coverage, "complete");
    println!(
        "physical fixture: 192 directories, 1536 files, scan and verification {:?}",
        started.elapsed()
    );
    Ok(())
}
