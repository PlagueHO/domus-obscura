use native_sqlite::{Context, DirectoryChain, OpenMode, SELECTION_FILE, SelectionFence};
use std::fs::{File, OpenOptions};
use std::path::Path;
use std::sync::{Arc, Barrier};
use std::time::Duration;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn directory(path: &Path) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_DIRECTORY | libc::O_CLOEXEC);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
        };
        options.custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT);
    }
    options.open(path)
}

fn fixture() -> Result<(tempfile::TempDir, Context), Box<dyn std::error::Error>> {
    let root = tempfile::Builder::new()
        .prefix(".native-sqlite-")
        .tempdir_in(".")?;
    std::fs::create_dir(root.path().join("state"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(root.path().join("state").join("sqlite-tmp"))?;
    }
    #[cfg(windows)]
    std::fs::create_dir(root.path().join("state").join("sqlite-tmp"))?;
    let context = context(root.path())?;
    Ok((root, context))
}

fn context(root: &Path) -> Result<Context, Box<dyn std::error::Error>> {
    let chain = DirectoryChain::new(
        directory(root)?,
        vec![("state".into(), directory(&root.join("state"))?)],
    )?;
    Ok(Context::new(
        chain,
        directory(&root.join("state").join("sqlite-tmp"))?,
        "state.sqlite",
    )?)
}

fn selection_file(root: &Path, create: bool) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create_new(create);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.share_mode(3);
    }
    options.open(root.join("state").join(SELECTION_FILE))
}

fn seed_selection(root: &Path) -> std::io::Result<()> {
    use std::io::Write;
    let mut file = selection_file(root, true)?;
    file.write_all(&SelectionFence::initial_record())?;
    file.sync_all()
}

fn fenced_context(root: &Path, token: &str) -> native_sqlite::Result<Context> {
    let fence = SelectionFence::acquire(selection_file(root, false)?, token)?;
    Context::new_fenced(
        DirectoryChain::new(
            directory(root)?,
            vec![("state".into(), directory(&root.join("state"))?)],
        )?,
        directory(&root.join("state").join("sqlite-tmp"))?,
        "state.sqlite",
        fence,
    )
}

#[test]
fn selection_fence_shares_only_the_exact_snapshot_and_never_repairs_damaged_records() -> TestResult
{
    let (root, original) = fixture()?;
    drop(original);
    seed_selection(root.path())?;
    let first = fenced_context(root.path(), &"a".repeat(64))?;
    let same = fenced_context(root.path(), &"a".repeat(64))?;
    assert!(matches!(
        fenced_context(root.path(), &"b".repeat(64)),
        Err(native_sqlite::Error::SelectionInUse)
    ));
    drop(first);
    assert!(matches!(
        fenced_context(root.path(), &"b".repeat(64)),
        Err(native_sqlite::Error::SelectionInUse)
    ));
    drop(same);
    drop(fenced_context(root.path(), &"b".repeat(64))?);
    let path = root.path().join("state").join(SELECTION_FILE);
    let valid = std::fs::read(&path)?;
    let mut checksum = valid.clone();
    checksum[valid.len() - 3] ^= 1;
    for damaged in [valid[..valid.len() - 1].to_vec(), checksum, vec![b'x'; 400]] {
        std::fs::write(&path, &damaged)?;
        assert!(matches!(
            fenced_context(root.path(), &"a".repeat(64)),
            Err(native_sqlite::Error::SelectionRecord)
        ));
        assert_eq!(std::fs::read(&path)?, damaged);
    }
    Ok(())
}

#[test]
fn selection_fence_refuses_conflicting_process_ownership() -> TestResult {
    const CHILD: &str = "DOMUS_SQLITE_SELECTION_CHILD";
    if let Some(root) = std::env::var_os(CHILD) {
        assert!(matches!(
            fenced_context(Path::new(&root), &"b".repeat(64)),
            Err(native_sqlite::Error::SelectionInUse)
        ));
        return Ok(());
    }
    let (root, original) = fixture()?;
    drop(original);
    seed_selection(root.path())?;
    let owner = fenced_context(root.path(), &"a".repeat(64))?;
    bounded_child(
        "selection_fence_refuses_conflicting_process_ownership",
        CHILD,
        root.path(),
    )?;
    drop(owner);
    drop(fenced_context(root.path(), &"b".repeat(64))?);
    Ok(())
}

#[cfg(unix)]
#[test]
fn selection_fence_replacement_refuses_native_sqlite_effects() -> TestResult {
    let (root, original) = fixture()?;
    drop(original);
    seed_selection(root.path())?;
    let owner = fenced_context(root.path(), &"a".repeat(64))?;
    let db = owner.open(OpenMode::Create)?;
    configure(&db)?;
    db.execute_batch("CREATE TABLE sample(value); INSERT INTO sample VALUES(1)")?;
    let path = root.path().join("state").join(SELECTION_FILE);
    let bytes = std::fs::read(&path)?;
    std::fs::rename(&path, path.with_extension("retained"))?;
    seed_selection(root.path())?;
    std::fs::write(&path, &bytes)?;
    assert!(owner.check().is_err());
    assert!(db.execute("INSERT INTO sample VALUES(2)", []).is_err());
    let ordinary = rusqlite::Connection::open(root.path().join("state").join("state.sqlite"))?;
    assert_eq!(
        ordinary.query_row("SELECT count(*) FROM sample", [], |row| row
            .get::<_, i64>(0))?,
        1
    );
    Ok(())
}

fn configure(db: &native_sqlite::ScopedConnection) -> native_sqlite::Result<()> {
    db.busy_timeout(Duration::from_secs(5))?;
    db.execute_batch("PRAGMA foreign_keys=ON; PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;")?;
    Ok(())
}

#[test]
fn independent_owners_keep_snapshot_and_native_contention() -> TestResult {
    let (root, first) = fixture()?;
    let writer = first.open(OpenMode::Create)?;
    configure(&writer)?;
    writer.execute_batch(
        "CREATE TABLE sample(id INTEGER PRIMARY KEY); INSERT INTO sample VALUES(1);",
    )?;
    let second = context(root.path())?;
    let reader = second.open(OpenMode::ReadOnly)?;
    reader.busy_timeout(Duration::from_secs(5))?;
    reader.execute_batch("BEGIN")?;
    assert_eq!(
        reader.query_row("SELECT count(*) FROM sample", [], |r| r.get::<_, i64>(0))?,
        1
    );
    let barrier = Arc::new(Barrier::new(4));
    let mut workers = Vec::new();
    for (index, scope) in [(0, first.clone()), (1, second.clone())] {
        let barrier = barrier.clone();
        workers.push(std::thread::spawn(move || -> native_sqlite::Result<()> {
            let ready = scope.open(OpenMode::ReadWrite).and_then(|db| {
                configure(&db)?;
                Ok(db)
            });
            barrier.wait();
            let db = ready?;
            for value in 0..20 {
                db.execute("INSERT INTO sample VALUES(?1)", [2 + index * 20 + value])?;
            }
            db.close()
        }));
    }
    let ordinary_path = root.path().join("ordinary.sqlite");
    let ordinary_barrier = barrier.clone();
    workers.push(std::thread::spawn(move || -> native_sqlite::Result<()> {
        let ready = rusqlite::Connection::open(ordinary_path);
        ordinary_barrier.wait();
        let db = ready?;
        db.execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE ordinary(value)")?;
        for value in 0..20 {
            db.execute("INSERT INTO ordinary VALUES(?1)", [value])?;
        }
        Ok(())
    }));
    barrier.wait();
    for worker in workers {
        worker.join().map_err(|_| "worker panicked")??;
    }
    assert_eq!(
        reader.query_row("SELECT count(*) FROM sample", [], |r| r.get::<_, i64>(0))?,
        1
    );
    reader.execute_batch("COMMIT")?;
    assert_eq!(
        reader.query_row("SELECT count(*) FROM sample", [], |r| r.get::<_, i64>(0))?,
        41
    );
    reader.close()?;
    writer.close()?;
    assert_eq!(first.statistics().live_native_handles, 0);
    Ok(())
}

const TEMP_SEED: &str = "
 PRAGMA temp_store=FILE; PRAGMA temp.cache_size=8;
 CREATE TEMP TABLE spill(id INTEGER PRIMARY KEY,payload TEXT);
 WITH RECURSIVE source(x) AS(VALUES(1) UNION ALL SELECT x+1 FROM source WHERE x<4096)
 INSERT INTO spill SELECT x,printf('%08d',4097-x)||hex(zeroblob(512)) FROM source;";

#[test]
fn real_temp_and_sort_spill_use_only_private_child() -> TestResult {
    let (root, context) = fixture()?;
    let db = context.open(OpenMode::Create)?;
    configure(&db)?;
    db.execute_batch(TEMP_SEED)?;
    let before = context.statistics();
    assert!(before.temporary_opens > 0 && before.temporary_bytes_written > 0);
    {
        let mut statement = db.prepare("SELECT payload FROM spill ORDER BY payload")?;
        let mut rows = statement.query([])?;
        let mut count = 0;
        while let Some(row) = rows.next()? {
            let payload: String = row.get(0)?;
            count += 1;
            assert!(payload.starts_with(&format!("{count:08}")));
        }
        assert_eq!(count, 4096);
    }
    let after = context.statistics();
    assert!(after.temporary_opens > before.temporary_opens);
    assert!(after.temporary_bytes_written > before.temporary_bytes_written);
    assert!(
        db.execute_batch("ATTACH 'other.sqlite' AS forbidden")
            .is_err()
    );
    db.execute_batch("ATTACH '' AS scratch; CREATE TABLE scratch.private(value); DETACH scratch;")?;
    db.execute_batch("VACUUM")?;
    assert!(db.execute_batch("PRAGMA temp_store_directory='.'").is_err());
    assert!(!root.path().join("state").join("other.sqlite").exists());
    db.close()?;
    assert_eq!(
        std::fs::read_dir(root.path().join("state").join("sqlite-tmp"))?.count(),
        0
    );
    assert_eq!(context.statistics().live_native_handles, 0);
    Ok(())
}

#[cfg(unix)]
#[test]
fn replaced_state_and_temporary_namespace_refuse_new_effects() -> TestResult {
    for replace_temp in [false, true] {
        let (root, context) = fixture()?;
        let db = context.open(OpenMode::Create)?;
        configure(&db)?;
        db.execute_batch("CREATE TABLE sample(value); INSERT INTO sample VALUES(1)")?;
        let selected = if replace_temp {
            root.path().join("state").join("sqlite-tmp")
        } else {
            root.path().join("state")
        };
        let moved = root.path().join("retained");
        std::fs::rename(&selected, &moved)?;
        std::fs::create_dir(&selected)?;
        let sentinel = selected.join("state.sqlite");
        std::fs::write(&sentinel, b"replacement must not change")?;
        assert!(context.open(OpenMode::Create).is_err());
        assert!(db.execute_batch("INSERT INTO sample VALUES(2)").is_err());
        assert_eq!(std::fs::read(&sentinel)?, b"replacement must not change");
        drop(db);
    }
    Ok(())
}

#[cfg(unix)]
#[test]
fn linked_main_and_sidecar_do_not_touch_target() -> TestResult {
    use std::os::unix::fs::symlink;
    for leaf in [
        "state.sqlite",
        "state.sqlite-wal",
        "state.sqlite-shm",
        "state.sqlite-journal",
    ] {
        let (root, scope) = fixture()?;
        if leaf != "state.sqlite" {
            let db = scope.open(OpenMode::Create)?;
            configure(&db)?;
            db.execute_batch("CREATE TABLE sample(value)")?;
            db.close()?;
        }
        let target = root.path().join("sentinel");
        std::fs::write(&target, b"do not change")?;
        symlink(
            std::fs::canonicalize(&target)?,
            root.path().join("state").join(leaf),
        )?;
        let result = scope.open(OpenMode::ReadWrite).and_then(|db| {
            db.execute_batch(
                "SELECT * FROM sqlite_schema; BEGIN IMMEDIATE; CREATE TABLE forbidden(value)",
            )?;
            Ok(())
        });
        assert!(result.is_err(), "{leaf} must refuse");
        assert_eq!(std::fs::read(target)?, b"do not change");
    }
    Ok(())
}

#[test]
fn raw_close_v2_dependents_keep_native_authority() -> TestResult {
    // Raw API use is confined to this lifetime regression. Safe statements
    // cannot outlive their borrowed rusqlite Connection.
    let (root, original) = fixture()?;
    drop(original);
    seed_selection(root.path())?;
    let scope = fenced_context(root.path(), &"a".repeat(64))?;
    let db = scope.open(OpenMode::Create)?;
    configure(&db)?;
    db.execute_batch(TEMP_SEED)?;
    let raw = unsafe { db.handle() };
    let destination = rusqlite::Connection::open_in_memory()?;
    let mut blob = std::ptr::null_mut();
    assert_eq!(
        unsafe {
            rusqlite::ffi::sqlite3_blob_open(
                raw,
                c"temp".as_ptr(),
                c"spill".as_ptr(),
                c"payload".as_ptr(),
                1,
                0,
                &mut blob,
            )
        },
        rusqlite::ffi::SQLITE_OK
    );
    let backup = unsafe {
        rusqlite::ffi::sqlite3_backup_init(
            destination.handle(),
            c"main".as_ptr(),
            raw,
            c"temp".as_ptr(),
        )
    };
    assert!(!backup.is_null());
    assert_eq!(
        unsafe { rusqlite::ffi::sqlite3_backup_step(backup, 1) },
        rusqlite::ffi::SQLITE_OK
    );
    assert!(unsafe { rusqlite::ffi::sqlite3_backup_remaining(backup) } > 0);
    let mut statement = std::ptr::null_mut();
    let rc = unsafe {
        rusqlite::ffi::sqlite3_prepare_v2(
            raw,
            c"SELECT sum(id) FROM temp.spill".as_ptr(),
            -1,
            &mut statement,
            std::ptr::null_mut(),
        )
    };
    assert_eq!(rc, rusqlite::ffi::SQLITE_OK);
    // The boundary reports BUSY, then uses close_v2 for raw dependents.
    assert!(db.close().is_err());
    drop(scope);
    assert!(matches!(
        fenced_context(root.path(), &"b".repeat(64)),
        Err(native_sqlite::Error::SelectionInUse)
    ));
    assert_eq!(
        unsafe { rusqlite::ffi::sqlite3_step(statement) },
        rusqlite::ffi::SQLITE_ROW
    );
    assert_eq!(
        unsafe { rusqlite::ffi::sqlite3_column_int(statement, 0) },
        8_390_656
    );
    assert_eq!(
        unsafe { rusqlite::ffi::sqlite3_finalize(statement) },
        rusqlite::ffi::SQLITE_OK
    );
    let mut prefix = [0_u8; 8];
    assert_eq!(
        unsafe { rusqlite::ffi::sqlite3_blob_read(blob, prefix.as_mut_ptr().cast(), 8, 0) },
        rusqlite::ffi::SQLITE_OK
    );
    assert_eq!(&prefix, b"00004096");
    assert_eq!(
        unsafe { rusqlite::ffi::sqlite3_blob_close(blob) },
        rusqlite::ffi::SQLITE_OK
    );
    assert!(matches!(
        fenced_context(root.path(), &"b".repeat(64)),
        Err(native_sqlite::Error::SelectionInUse)
    ));
    assert_eq!(
        unsafe { rusqlite::ffi::sqlite3_backup_step(backup, -1) },
        rusqlite::ffi::SQLITE_DONE
    );
    assert_eq!(
        unsafe { rusqlite::ffi::sqlite3_backup_finish(backup) },
        rusqlite::ffi::SQLITE_OK
    );
    assert_eq!(
        destination.query_row("SELECT count(*) FROM spill", [], |row| row.get::<_, i64>(0))?,
        4096
    );
    let fresh = fenced_context(root.path(), &"b".repeat(64))?;
    assert_eq!(fresh.statistics().live_native_handles, 0);
    fresh.close()?;
    Ok(())
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
#[test]
fn refused_native_close_quarantines_in_owned_child() -> TestResult {
    const CHILD: &str = "DOMUS_SQLITE_LINUX_CLOSE_CHILD";
    if let Some(root) = std::env::var_os(CHILD) {
        let root = std::path::PathBuf::from(root);
        seed_selection(&root)?;
        let scope = fenced_context(&root, &"a".repeat(64))?;
        let db = scope.open(OpenMode::Create)?;
        configure(&db)?;
        db.execute_batch(TEMP_SEED)?;
        let expected = std::fs::canonicalize(root.join("state").join("sqlite-tmp"))?;
        let mut selected = None;
        for entry in std::fs::read_dir("/proc/self/fd")? {
            let entry = entry?;
            if let Ok(target) = std::fs::read_link(entry.path())
                && target.parent() == Some(expected.as_path())
                && target
                    .file_name()
                    .and_then(|v| v.to_str())
                    .is_some_and(|v| v.starts_with("etilqs_"))
            {
                selected = Some(entry.file_name().to_string_lossy().parse::<i32>()?);
                break;
            }
        }
        let fd = selected.ok_or("no owned SQLite TEMP descriptor")?;
        let filter = [
            libc::sock_filter {
                code: 0x20,
                jt: 0,
                jf: 0,
                k: 4,
            },
            libc::sock_filter {
                code: 0x15,
                jt: 1,
                jf: 0,
                k: 0xc000003e,
            },
            libc::sock_filter {
                code: 0x06,
                jt: 0,
                jf: 0,
                k: 0x80000000,
            },
            libc::sock_filter {
                code: 0x20,
                jt: 0,
                jf: 0,
                k: 0,
            },
            libc::sock_filter {
                code: 0x15,
                jt: 0,
                jf: 3,
                k: libc::SYS_close as u32,
            },
            libc::sock_filter {
                code: 0x20,
                jt: 0,
                jf: 0,
                k: 16,
            },
            libc::sock_filter {
                code: 0x15,
                jt: 0,
                jf: 1,
                k: fd as u32,
            },
            libc::sock_filter {
                code: 0x06,
                jt: 0,
                jf: 0,
                k: 0x00050000 | libc::EPERM as u32,
            },
            libc::sock_filter {
                code: 0x06,
                jt: 0,
                jf: 0,
                k: 0x7fff0000,
            },
        ];
        let program = libc::sock_fprog {
            len: filter.len() as u16,
            filter: filter.as_ptr().cast_mut(),
        };
        assert_eq!(
            unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) },
            0
        );
        assert_eq!(
            unsafe { libc::prctl(libc::PR_SET_SECCOMP, 2, &program as *const libc::sock_fprog) },
            0
        );
        assert!(db.close().is_err());
        assert!(scope.statistics().quarantined);
        assert!(scope.statistics().live_native_handles > 0);
        assert!(scope.open(OpenMode::Create).is_err());
        assert!(unsafe { libc::fcntl(fd, libc::F_GETFD) } >= 0);
        drop(scope);
        assert!(matches!(
            fenced_context(&root, &"b".repeat(64)),
            Err(native_sqlite::Error::SelectionInUse)
        ));
        // The filter belongs to this bounded child only. Do not retry close.
        return Ok(());
    }
    let (root, scope) = fixture()?;
    drop(scope);
    bounded_child(
        "refused_native_close_quarantines_in_owned_child",
        CHILD,
        root.path(),
    )
}

#[cfg(windows)]
#[test]
fn replaced_temporary_namespace_and_wrong_types_are_refused() -> TestResult {
    let (root, scope) = fixture()?;
    let db = scope.open(OpenMode::Create)?;
    configure(&db)?;
    let temporary = root.path().join("state").join("sqlite-tmp");
    // Test pins allow delete sharing. A real production parent may instead
    // deny rename with its original retained handles.
    std::fs::rename(&temporary, root.path().join("retained-tmp"))?;
    std::fs::create_dir(&temporary)?;
    let sentinel = temporary.join("sentinel");
    std::fs::write(&sentinel, b"unchanged")?;
    assert!(db.execute_batch(TEMP_SEED).is_err());
    assert!(scope.open(OpenMode::Create).is_err());
    assert_eq!(std::fs::read(&sentinel)?, b"unchanged");
    drop(db);
    drop(scope);
    let (root, scope) = fixture()?;
    std::fs::create_dir(root.path().join("state").join("state.sqlite"))?;
    assert!(scope.open(OpenMode::Create).is_err());
    assert!(root.path().join("state").join("state.sqlite").is_dir());
    Ok(())
}

#[cfg(windows)]
#[test]
fn temporary_junction_does_not_authorize_target() -> TestResult {
    let (root, scope) = fixture()?;
    let temporary = root.path().join("state").join("sqlite-tmp");
    std::fs::rename(&temporary, root.path().join("retained-tmp"))?;
    let target = root.path().join("unapproved");
    std::fs::create_dir(&target)?;
    let sentinel = target.join("sentinel");
    std::fs::write(&sentinel, b"unchanged")?;
    let status = std::process::Command::new("powershell.exe")
        .args([
            "-NoProfile", "-NonInteractive", "-Command",
            "$ErrorActionPreference='Stop'; New-Item -ItemType Junction -Path $env:DOMUS_LINK -Target $env:DOMUS_TARGET | Out-Null",
        ])
        .env("DOMUS_LINK", std::fs::canonicalize(root.path().join("state"))?.join("sqlite-tmp"))
        .env("DOMUS_TARGET", std::fs::canonicalize(&target)?)
        .status()?;
    assert!(status.success());
    assert!(scope.open(OpenMode::Create).is_err());
    assert!(context(root.path()).is_err());
    assert_eq!(std::fs::read(&sentinel)?, b"unchanged");
    assert_eq!(std::fs::read_dir(&target)?.count(), 1);
    // Remove only the generated junction. Never traverse its target for cleanup.
    std::fs::remove_dir(&temporary)?;
    Ok(())
}

#[cfg(windows)]
#[test]
fn protected_native_close_quarantines_in_owned_child() -> TestResult {
    const CHILD: &str = "DOMUS_SQLITE_CLOSE_CHILD";
    if std::env::var_os(CHILD).is_some() {
        use windows_sys::Win32::Foundation::{
            HANDLE_FLAG_PROTECT_FROM_CLOSE, SetHandleInformation,
        };
        let root = std::path::PathBuf::from(std::env::var_os(CHILD).ok_or("missing child root")?);
        seed_selection(&root)?;
        let scope = fenced_context(&root, &"a".repeat(64))?;
        let db = scope.open(OpenMode::Create)?;
        configure(&db)?;
        db.execute_batch(TEMP_SEED)?;
        let mut handle = std::ptr::null_mut::<std::ffi::c_void>();
        assert_eq!(
            unsafe {
                rusqlite::ffi::sqlite3_file_control(
                    db.handle(),
                    c"temp".as_ptr(),
                    rusqlite::ffi::SQLITE_FCNTL_WIN32_GET_HANDLE,
                    (&mut handle as *mut *mut std::ffi::c_void).cast(),
                )
            },
            rusqlite::ffi::SQLITE_OK
        );
        assert!(!handle.is_null());
        assert_ne!(
            unsafe {
                SetHandleInformation(
                    handle,
                    HANDLE_FLAG_PROTECT_FROM_CLOSE,
                    HANDLE_FLAG_PROTECT_FROM_CLOSE,
                )
            },
            0
        );
        assert!(db.close().is_err());
        assert!(scope.statistics().quarantined);
        assert!(scope.open(OpenMode::Create).is_err());
        let mut flags = 0;
        assert_ne!(
            unsafe { windows_sys::Win32::Foundation::GetHandleInformation(handle, &mut flags) },
            0
        );
        assert_ne!(flags & HANDLE_FLAG_PROTECT_FROM_CLOSE, 0);
        drop(scope);
        assert!(matches!(
            fenced_context(&root, &"b".repeat(64)),
            Err(native_sqlite::Error::SelectionInUse)
        ));
        // Only this child owns the failed native handle. Process exit releases
        // it. Do not retry close or add a runtime recovery path.
        return Ok(());
    }
    let (root, scope) = fixture()?;
    drop(scope);
    bounded_child(
        "protected_native_close_quarantines_in_owned_child",
        CHILD,
        root.path(),
    )
}

fn bounded_child(test: &str, variable: &str, root: &Path) -> TestResult {
    let mut child = std::process::Command::new(std::env::current_exe()?)
        .arg("--exact")
        .arg(test)
        .env(variable, std::fs::canonicalize(root)?)
        .spawn()?;
    let start = std::time::Instant::now();
    loop {
        if let Some(status) = child.try_wait()? {
            assert!(status.success());
            break;
        }
        if start.elapsed() > Duration::from_secs(30) {
            child.kill()?;
            return Err("native close child exceeded 30 seconds".into());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    Ok(())
}
