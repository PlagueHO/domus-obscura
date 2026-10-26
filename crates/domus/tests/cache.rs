use domus::application::{
    add_location,
    services::cache::{Cache, CacheKey, CacheLookup},
};
use domus::infrastructure::{
    cache::{CacheLimits, FileCache},
    staging::StagingRoot,
    store::Store,
};
use sha2::{Digest, Sha256};
use std::io::{Seek, SeekFrom, Write};
use std::path::PathBuf;

#[path = "support/private.rs"]
mod private;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

struct Fixture {
    store: Store,
    root: PathBuf,
    _state: tempfile::TempDir,
    _files: tempfile::TempDir,
}

fn limits() -> CacheLimits {
    CacheLimits {
        item_bytes: 1024,
        total_bytes: 4096,
        entries: 3,
    }
}

fn key(value: &str) -> TestResult<CacheKey> {
    Ok(CacheKey::new("generated-provider", "originals", value)?)
}

fn entry_path(fixture: &Fixture, key: &CacheKey) -> PathBuf {
    let digest: String = key
        .digest()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    fixture.root.join(format!("{digest}.entry"))
}

impl Fixture {
    fn new() -> TestResult<Self> {
        let state = private::tempdir()?;
        let files = private::tempdir()?;
        let root = files.path().join("cache");
        std::fs::create_dir(&root)?;
        private::directory(&root)?;
        Ok(Self {
            store: Store::open(&state.path().join("state.db"))?,
            root,
            _state: state,
            _files: files,
        })
    }

    fn open(&mut self) -> TestResult<FileCache> {
        Ok(FileCache::open(&mut self.store, &self.root, limits())?)
    }
}

#[test]
fn shared_cache_keys_are_scoped_bounded_and_never_native_paths() -> TestResult {
    let first = CacheKey::new("provider-a", "images", "../../outside:stream\\image")?;
    assert_ne!(
        first,
        CacheKey::new("provider-b", "images", "../../outside:stream\\image")?
    );
    assert_ne!(
        first,
        CacheKey::new("provider-a", "variants", "../../outside:stream\\image")?
    );
    assert_ne!(
        CacheKey::new("ab", "c", "d")?,
        CacheKey::new("a", "bc", "d")?
    );
    for (provider, namespace, id) in [
        ("", "images", "id"),
        ("Provider", "images", "id"),
        ("p", "../images", "id"),
        ("p", "images", ""),
        ("p", "images", "bad\nid"),
    ] {
        assert!(CacheKey::new(provider, namespace, id).is_err());
    }
    assert!(key(&"x".repeat(4097)).is_err());
    Ok(())
}

#[tokio::test]
async fn shared_cache_host_configuration_is_explicit_bounded_and_never_migrates_startup()
-> TestResult {
    use domus::infrastructure::services::load_host;
    use serde_json::json;
    let mut fixture = Fixture::new()?;
    let host_path = fixture._state.path().join("host.json");
    let write = |value: serde_json::Value| -> TestResult {
        std::fs::write(&host_path, serde_json::to_vec(&value)?)?;
        Ok(())
    };
    write(json!({"database":fixture.store.database_path()}))?;
    drop(load_host(&host_path)?);
    assert!(!fixture.root.join("owner.lock").exists());
    for cache in [
        json!({"root":"relative-cache"}),
        json!({"root":fixture.root,"limits":{"item_bytes":0}}),
        json!({"root":fixture.root,"limits":{"entries":4097}}),
        json!({"root":fixture.root,"unexpected":true}),
        json!({"root":fixture.root,"limits":{"unknown":1}}),
        json!({"root":fixture._state.path()}),
    ] {
        write(json!({"database":fixture.store.database_path(),"cache":cache}))?;
        assert!(load_host(&host_path).is_err());
        assert!(!fixture.root.join("owner.lock").exists());
    }
    write(json!({"database":fixture.store.database_path(),"cache":{"root":fixture.root}}))?;
    drop(load_host(&host_path)?);
    assert!(fixture.root.join("owner.lock").exists());
    drop(load_host(&host_path)?);
    let raw = rusqlite::Connection::open(fixture.store.database_path())?;
    raw.execute("UPDATE catalog_gate SET next_request_ms=9223372036854775807,delay_seconds=4294967295 WHERE id=1", [])?;
    let release = "20000000-0000-4000-8000-000000000001";
    let source = format!("https://coverartarchive.org/release/{release}/");
    let body = serde_json::to_vec(
        &json!({"release":format!("https://musicbrainz.org/release/{release}"),"images":[]}),
    )?;
    let sources = serde_json::to_vec(&vec![source.clone()])?;
    let mut envelope = b"DOMCAA1\n".to_vec();
    envelope.extend_from_slice(&(sources.len() as u32).to_le_bytes());
    envelope.extend_from_slice(&sources);
    envelope.extend_from_slice(&body);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs();
    let cache = FileCache::open(&mut fixture.store, &fixture.root, CacheLimits::default())?;
    cache.put(
        &CacheKey::new("cover-art-archive", "responses-v1", &source)?,
        &envelope,
        now,
        now + 86400,
    )?;
    let response = load_host(&host_path)?
        .artwork
        .fetch(domus::application::services::ArtworkRequest {
            provider: "cover-art-archive".into(),
            release: domus::application::services::EntityRef {
                provider: "musicbrainz".into(),
                id: release.into(),
            },
            image_id: None,
        })
        .await?;
    assert!(
        matches!(response.content, domus::application::services::ArtworkContent::Listing(images) if images.is_empty())
    );
    assert_eq!(
        raw.query_row(
            "SELECT count(*) FROM private_roots WHERE kind='cache'",
            [],
            |row| row.get::<_, i64>(0)
        )?,
        1
    );
    assert_eq!(
        raw.query_row(
            "SELECT generation FROM catalog_gate WHERE id=1",
            [],
            |row| row.get::<_, i64>(0)
        )?,
        0
    );
    let missing = fixture._state.path().join("missing.db");
    write(json!({"database":missing,"cache":{"root":fixture.root}}))?;
    assert!(load_host(&host_path).is_err());
    assert!(!missing.exists());
    let old = fixture._state.path().join("legacy.db");
    let old_db = rusqlite::Connection::open(&old)?;
    old_db.execute_batch("PRAGMA user_version=20; CREATE TABLE sentinel(value TEXT); INSERT INTO sentinel VALUES('retained');")?;
    write(json!({"database":old,"cache":{"root":fixture.root}}))?;
    assert!(load_host(&host_path).is_err());
    assert_eq!(
        old_db.pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))?,
        20
    );
    assert_eq!(
        old_db.query_row("SELECT value FROM sentinel", [], |row| row
            .get::<_, String>(0))?,
        "retained"
    );
    let _ = fixture.open()?;
    Ok(())
}

#[test]
fn shared_cache_distinguishes_miss_expiry_corruption_and_storage_failure() -> TestResult {
    let mut fixture = Fixture::new()?;
    let cache = fixture.open()?;
    let key = key("generated original")?;
    assert!(matches!(cache.lookup(&key, 10)?, CacheLookup::Miss));
    let digest = cache.put(&key, b"generated bytes", 10, 20)?;
    assert_eq!(digest, <[u8; 32]>::from(Sha256::digest(b"generated bytes")));
    let CacheLookup::Hit(hit) = cache.lookup(&key, 19)? else {
        return Err("cache hit missing".into());
    };
    assert_eq!(hit.bytes, b"generated bytes");
    assert_eq!(hit.sha256, digest);
    assert_eq!((hit.stored_at, hit.expires_at), (10, 20));
    assert!(matches!(cache.lookup(&key, 20)?, CacheLookup::Expired));
    assert!(matches!(cache.lookup(&key, 9)?, CacheLookup::Expired));
    assert!(cache.put(&key, &[1; 1025], 10, 20).is_err());
    assert!(cache.put(&key, b"invalid expiry", 20, 20).is_err());
    assert!(matches!(cache.lookup(&key, 19)?, CacheLookup::Hit(_)));
    assert_eq!(cache.evict_expired(20, 1)?, 1);
    assert!(!cache.invalidate(&key)?);
    cache.put(&key, b"generated bytes", 10, 20)?;
    let path = entry_path(&fixture, &key);
    let mut file = std::fs::OpenOptions::new().write(true).open(&path)?;
    file.seek(SeekFrom::Start(96))?;
    file.write_all(b"X")?;
    drop(file);
    assert!(matches!(cache.lookup(&key, 19)?, CacheLookup::Corrupt));
    drop(cache);
    let cache = fixture.open()?;
    assert!(matches!(cache.lookup(&key, 19)?, CacheLookup::Corrupt));
    assert!(cache.invalidate(&key)?);
    cache.put(&key, b"header", 10, 20)?;
    std::fs::OpenOptions::new()
        .write(true)
        .open(&path)?
        .set_len(10)?;
    assert!(matches!(cache.lookup(&key, 19)?, CacheLookup::Corrupt));
    assert!(cache.evict_expired(20, 1).is_err());
    assert!(cache.invalidate(&key)?);
    std::fs::create_dir(&path)?;
    assert!(
        cache.lookup(&key, 19).is_err(),
        "a directory is not a cache miss"
    );
    Ok(())
}

#[test]
fn shared_cache_bounds_storage_eviction_and_atomic_replacement() -> TestResult {
    let mut fixture = Fixture::new()?;
    let cache = fixture.open()?;
    for number in 1..=9 {
        cache.put(
            &key(&number.to_string())?,
            &[number; 1024],
            10,
            100 + u64::from(number),
        )?;
        let files = std::fs::read_dir(&fixture.root)?.collect::<Result<Vec<_>, _>>()?;
        assert!(files.len() <= 4);
        let total = files
            .iter()
            .filter(|file| file.file_name() != "owner.lock")
            .map(|file| file.metadata().map(|meta| meta.len()))
            .collect::<Result<Vec<_>, _>>()?
            .iter()
            .sum::<u64>();
        assert!(total <= limits().total_bytes);
    }
    assert!(matches!(cache.lookup(&key("1")?, 20)?, CacheLookup::Miss));
    let latest = key("9")?;
    cache.put(&latest, &[42; 1024], 11, 200)?;
    let CacheLookup::Hit(hit) = cache.lookup(&latest, 20)? else {
        return Err("replacement missing".into());
    };
    assert_eq!(hit.bytes, vec![42; 1024]);
    assert!(cache.evict_expired(300, 0).is_err());
    assert!(cache.evict_expired(300, 4).is_err());
    assert_eq!(cache.evict_expired(300, 1)?, 1);
    assert_eq!(cache.evict_expired(300, 3)?, 1);
    assert_eq!(std::fs::read_dir(&fixture.root)?.count(), 1);
    assert!(
        FileCache::open(
            &mut fixture.store,
            &fixture.root,
            CacheLimits {
                entries: 0,
                ..limits()
            }
        )
        .is_err()
    );
    Ok(())
}

#[test]
fn shared_cache_serializes_clones_and_excludes_other_owners() -> TestResult {
    let mut fixture = Fixture::new()?;
    let cache = fixture.open()?;
    let other_adapter = fixture.open()?;
    let key = key("shared concurrent key")?;
    cache.put(&key, &[0; 1024], 10, 30)?;
    assert!(matches!(
        other_adapter.lookup(&key, 20)?,
        CacheLookup::Hit(_)
    ));
    let owner = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(fixture.root.join("owner.lock"))?;
    owner.try_lock()?;
    assert!(matches!(cache.lookup(&key, 20), Err(domus::Error::Busy(_))));
    assert!(matches!(
        other_adapter.put(&key, b"must not write", 10, 30),
        Err(domus::Error::Busy(_))
    ));
    assert!(matches!(
        cache.evict_expired(30, 1),
        Err(domus::Error::Busy(_))
    ));
    assert!(
        fixture.open().is_err(),
        "startup must not discard a live writer's temporary file"
    );
    drop(owner);
    std::thread::scope(|scope| -> domus::Result<()> {
        let mut threads = Vec::new();
        for value in 1..=4 {
            let clone = cache.clone();
            let key = key.clone();
            threads.push(scope.spawn(move || -> domus::Result<()> {
                for _ in 0..15 {
                    clone.put(&key, &[value; 1024], 10, 30)?;
                    let CacheLookup::Hit(hit) = clone.lookup(&key, 20)? else {
                        return Err(domus::Error::Internal("concurrent cache hit missing"));
                    };
                    assert_eq!(hit.bytes.len(), 1024);
                    assert!(hit.bytes.iter().all(|byte| *byte == hit.bytes[0]));
                }
                Ok(())
            }));
        }
        for thread in threads {
            thread
                .join()
                .map_err(|_| domus::Error::Internal("cache test thread failed"))??;
        }
        Ok(())
    })?;
    drop(cache);
    drop(other_adapter);
    let reopened = fixture.open()?;
    assert!(matches!(reopened.lookup(&key, 20)?, CacheLookup::Hit(_)));
    Ok(())
}

#[test]
fn shared_cache_root_binding_protects_media_staging_and_copied_markers() -> TestResult {
    let mut fixture = Fixture::new()?;
    let cache = fixture.open()?;
    assert!(add_location(&fixture.store, &fixture.root, "main").is_err());
    let child = fixture.root.join("child");
    std::fs::create_dir(&child)?;
    assert!(add_location(&fixture.store, &child, "main").is_err());
    std::fs::remove_dir(&child)?;
    assert!(
        add_location(
            &fixture.store,
            fixture.root.parent().ok_or("parent missing")?,
            "main"
        )
        .is_err()
    );
    assert!(StagingRoot::open(&mut fixture.store, &fixture.root).is_err());
    drop(cache);
    let displaced = fixture.root.with_file_name("displaced");
    std::fs::rename(&fixture.root, &displaced)?;
    std::fs::create_dir(&fixture.root)?;
    private::directory(&fixture.root)?;
    std::fs::copy(
        displaced.join("owner.lock"),
        fixture.root.join("owner.lock"),
    )?;
    assert!(
        fixture.open().is_err(),
        "a copied marker must not bind a replacement root"
    );
    assert_eq!(std::fs::read_dir(&fixture.root)?.count(), 1);
    Ok(())
}

#[test]
fn shared_cache_refuses_media_roots_and_recovers_only_its_pending_file() -> TestResult {
    let mut fixture = Fixture::new()?;
    add_location(&fixture.store, &fixture.root, "main")?;
    assert!(fixture.open().is_err());
    assert!(!fixture.root.join("owner.lock").exists());
    let mut fixture = Fixture::new()?;
    let cache = fixture.open()?;
    let key = key("retained complete item")?;
    cache.put(&key, b"old complete bytes", 10, 30)?;
    drop(cache);
    std::fs::write(fixture.root.join("pending.cache"), b"interrupted write")?;
    let cache = fixture.open()?;
    assert!(!fixture.root.join("pending.cache").exists());
    let CacheLookup::Hit(hit) = cache.lookup(&key, 20)? else {
        return Err("old bytes lost".into());
    };
    assert_eq!(hit.bytes, b"old complete bytes");
    std::fs::write(fixture.root.join("unrelated"), b"must remain")?;
    assert!(cache.put(&key, b"new", 10, 30).is_err());
    assert_eq!(
        std::fs::read(fixture.root.join("unrelated"))?,
        b"must remain"
    );
    Ok(())
}

#[test]
fn shared_cache_keeps_bound_authority_when_root_replacement_is_attempted() -> TestResult {
    let mut fixture = Fixture::new()?;
    let cache = fixture.open()?;
    let key = key("retained root item")?;
    cache.put(&key, b"original", 10, 30)?;
    let displaced = fixture.root.with_file_name("displaced");
    let renamed = std::fs::rename(&fixture.root, &displaced);
    #[cfg(windows)]
    if let Err(error) = &renamed {
        assert_eq!(
            error.raw_os_error(),
            Some(32),
            "only the live sharing lock may prevent rename"
        );
        let CacheLookup::Hit(hit) = cache.lookup(&key, 20)? else {
            return Err("bound data lost".into());
        };
        assert_eq!(hit.bytes, b"original");
        return Ok(());
    }
    renamed?;
    std::fs::create_dir(&fixture.root)?;
    private::directory(&fixture.root)?;
    cache.put(&key, b"updated through bound handle", 10, 30)?;
    assert!(matches!(cache.lookup(&key, 20)?, CacheLookup::Hit(_)));
    assert_eq!(std::fs::read_dir(&fixture.root)?.count(), 0);
    drop(cache);
    assert!(fixture.open().is_err());
    assert_eq!(std::fs::read_dir(&fixture.root)?.count(), 0);
    Ok(())
}

#[test]
fn shared_cache_does_not_initialize_an_unowned_nonempty_root() -> TestResult {
    let mut fixture = Fixture::new()?;
    std::fs::write(fixture.root.join("evidence"), b"not cache data")?;
    assert!(fixture.open().is_err());
    assert_eq!(
        std::fs::read(fixture.root.join("evidence"))?,
        b"not cache data"
    );
    assert!(!fixture.root.join("owner.lock").exists());
    Ok(())
}

#[test]
fn shared_cache_child_process_probe() -> TestResult {
    let Some(root) = std::env::var_os("DOMUS_CACHE_TEST_ROOT") else {
        return Ok(());
    };
    let database = std::env::var_os("DOMUS_CACHE_TEST_DATABASE").ok_or("child database missing")?;
    let mut store = Store::open_current_existing(std::path::Path::new(&database))?;
    let cache = FileCache::open(&mut store, std::path::Path::new(&root), limits());
    if std::env::var_os("DOMUS_CACHE_TEST_BUSY").is_some() {
        assert!(matches!(cache, Err(domus::Error::Busy(_))));
    } else {
        let cache = cache?;
        let CacheLookup::Hit(hit) = cache.lookup(&key("cross-process")?, 20)? else {
            return Err("child cache hit missing".into());
        };
        assert_eq!(hit.bytes, b"retained cross-process bytes");
        cache.put(&key("child-write")?, b"child complete bytes", 10, 30)?;
    }
    Ok(())
}

#[test]
fn shared_cache_coordinates_processes_without_discarding_a_live_pending_write() -> TestResult {
    let mut fixture = Fixture::new()?;
    let cache = fixture.open()?;
    cache.put(
        &key("cross-process")?,
        b"retained cross-process bytes",
        10,
        30,
    )?;
    let owner = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(fixture.root.join("owner.lock"))?;
    owner.try_lock()?;
    std::fs::write(fixture.root.join("pending.cache"), b"live pending bytes")?;
    let run = |busy: bool| -> TestResult {
        let mut command = std::process::Command::new(std::env::current_exe()?);
        command
            .args(["--exact", "shared_cache_child_process_probe", "--nocapture"])
            .env("DOMUS_CACHE_TEST_ROOT", &fixture.root)
            .env("DOMUS_CACHE_TEST_DATABASE", fixture.store.database_path());
        if busy {
            command.env("DOMUS_CACHE_TEST_BUSY", "1");
        } else {
            command.env_remove("DOMUS_CACHE_TEST_BUSY");
        }
        let output = command.output()?;
        assert!(
            output.status.success(),
            "child probe failed: {}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        Ok(())
    };
    run(true)?;
    assert_eq!(
        std::fs::read(fixture.root.join("pending.cache"))?,
        b"live pending bytes"
    );
    drop(owner);
    run(false)?;
    assert!(!fixture.root.join("pending.cache").exists());
    let CacheLookup::Hit(hit) = cache.lookup(&key("child-write")?, 20)? else {
        return Err("parent cannot read child write".into());
    };
    assert_eq!(hit.bytes, b"child complete bytes");
    Ok(())
}

#[cfg(unix)]
#[test]
fn shared_cache_rejects_links_and_untrusted_permissions() -> TestResult {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let mut fixture = Fixture::new()?;
    let cache = fixture.open()?;
    let key = key("linked entry")?;
    let sentinel = fixture._state.path().join("sentinel");
    std::fs::write(&sentinel, b"must not change")?;
    let entry = entry_path(&fixture, &key);
    symlink(&sentinel, &entry)?;
    assert!(cache.lookup(&key, 20).is_err());
    assert!(cache.invalidate(&key).is_err());
    assert!(cache.put(&key, b"new", 10, 30).is_err());
    assert_eq!(std::fs::read(&sentinel)?, b"must not change");
    std::fs::remove_file(&entry)?;
    cache.put(&key, b"private", 10, 30)?;
    std::fs::set_permissions(&entry, std::fs::Permissions::from_mode(0o666))?;
    assert!(cache.lookup(&key, 20).is_err());
    drop(cache);
    std::fs::set_permissions(&fixture.root, std::fs::Permissions::from_mode(0o777))?;
    assert!(fixture.open().is_err());
    Ok(())
}
