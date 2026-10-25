use domus::domain::{LibraryPath, Page};
use domus::infrastructure::store::{Observation, Store};
use tempfile::tempdir;

/// Indexed-record/query evidence only. This does not measure physical scanning,
/// tag decoding, transfers, services, or combined process resident memory.
#[test]
#[ignore = "separate million-record scale check: cargo task scale"]
fn million_records_remain_paginated() -> Result<(), Box<dyn std::error::Error>> {
    let data = tempdir()?;
    let media = tempdir()?;
    let mut store = Store::open(&data.path().join("scale.db"))?;
    let root = std::fs::canonicalize(media.path())?;
    let location = store.add_location(root.to_str().ok_or("non-Unicode root")?, "main")?;
    let claim = store.begin_scan(location)?;
    let root = LibraryPath::parse("")?;
    for start in (0..1_000_000).step_by(128) {
        let paths = (start..(start + 128).min(1_000_000))
            .map(|index| LibraryPath::parse(&format!("track-{index:07}.mp3")))
            .collect::<domus::Result<Vec<_>>>()?;
        let batch = paths
            .iter()
            .map(|path| Observation {
                path,
                kind: "file",
                size: Some(1024),
                modified_at: Some(1),
            })
            .collect::<Vec<_>>();
        store.record_batch(&claim, &batch)?;
    }
    store.complete_directory(&claim, &root)?;
    assert!(store.finish_scan(&claim)?);
    let mut cursor = 0;
    let mut count = 0;
    loop {
        let page = store.browse(location, &root, Page::new(cursor, 200)?)?;
        assert!(page.len() <= 200);
        if page.is_empty() {
            break;
        }
        for entry in page {
            assert!(entry.id > cursor);
            cursor = entry.id;
            count += 1;
        }
    }
    assert_eq!(count, 1_000_000);
    Ok(())
}
