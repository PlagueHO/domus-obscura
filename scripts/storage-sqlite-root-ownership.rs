//! Isolated ownership proof using real SQLite and native directory handles.
//! The C fixture has one active context. `Rc` deliberately prevents sharing it
//! across threads; clients instead use a bounded owner-thread queue.
//! This is not a production callback registry or public ABI.

use std::cell::Cell;
use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::path::Path;
use std::ptr::{self, NonNull};
use std::rc::Rc;
use std::sync::{Arc, Barrier, mpsc};

type Database = c_void;
type Prepared = c_void;
type NativeBlob = c_void;
type NativeBackup = c_void;

unsafe extern "C" {
    fn domus_probe_root_new(path: *const c_char) -> *mut c_void;
    fn domus_probe_root_live(root: *mut c_void) -> c_int;
    fn domus_probe_root_drop(root: *mut c_void) -> c_int;
    fn domus_probe_connection(root: *mut c_void) -> *mut Database;
    fn domus_probe_seed(db: *mut Database);
    fn domus_probe_temporary_live() -> c_int;
    fn domus_probe_roots_released() -> c_int;
    fn domus_probe_native_close_errors() -> c_int;
    fn domus_probe_protect_temporary(db: *mut Database);
    fn domus_probe_verify_quarantine() -> c_int;
    fn domus_probe_recover_fixture();
    #[cfg(unix)]
    fn domus_probe_pins_live() -> c_int;
    #[cfg(unix)]
    fn domus_probe_pins_closed() -> c_int;
    fn sqlite3_open(name: *const c_char, db: *mut *mut Database) -> c_int;
    fn sqlite3_close(db: *mut Database) -> c_int;
    fn sqlite3_close_v2(db: *mut Database) -> c_int;
    fn sqlite3_prepare_v2(
        db: *mut Database,
        sql: *const c_char,
        length: c_int,
        statement: *mut *mut Prepared,
        tail: *mut *const c_char,
    ) -> c_int;
    fn sqlite3_step(statement: *mut Prepared) -> c_int;
    fn sqlite3_column_int(statement: *mut Prepared, column: c_int) -> c_int;
    fn sqlite3_column_text(statement: *mut Prepared, column: c_int) -> *const u8;
    fn sqlite3_column_bytes(statement: *mut Prepared, column: c_int) -> c_int;
    fn sqlite3_finalize(statement: *mut Prepared) -> c_int;
    fn sqlite3_blob_open(
        db: *mut Database,
        schema: *const c_char,
        table: *const c_char,
        column: *const c_char,
        row: i64,
        write: c_int,
        blob: *mut *mut NativeBlob,
    ) -> c_int;
    fn sqlite3_blob_read(
        blob: *mut NativeBlob,
        data: *mut c_void,
        n: c_int,
        offset: c_int,
    ) -> c_int;
    fn sqlite3_blob_close(blob: *mut NativeBlob) -> c_int;
    fn sqlite3_backup_init(
        destination: *mut Database,
        destination_schema: *const c_char,
        source: *mut Database,
        source_schema: *const c_char,
    ) -> *mut NativeBackup;
    fn sqlite3_backup_step(backup: *mut NativeBackup, pages: c_int) -> c_int;
    fn sqlite3_backup_remaining(backup: *mut NativeBackup) -> c_int;
    fn sqlite3_backup_finish(backup: *mut NativeBackup) -> c_int;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Release {
    Pending,
    Complete,
    Quarantined(c_int),
}

struct NativeRoot {
    raw: NonNull<c_void>,
    release: Rc<Cell<Release>>,
}

impl NativeRoot {
    fn bind(path: &Path) -> Rc<Self> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            std::fs::DirBuilder::new()
                .mode(0o700)
                .create(path)
                .expect("fresh private generated fixture");
        }
        #[cfg(windows)]
        std::fs::create_dir(path).expect("fresh generated fixture");
        let path = CString::new(path.to_str().expect("UTF-8 fixture path")).unwrap();
        let native = unsafe { domus_probe_root_new(path.as_ptr()) };
        Rc::new(Self {
            raw: NonNull::new(native).expect("real native root"),
            release: Rc::new(Cell::new(Release::Pending)),
        })
    }

    fn assert_live(&self) {
        assert_eq!(unsafe { domus_probe_root_live(self.raw.as_ptr()) }, 1);
        assert!(unsafe { domus_probe_temporary_live() } > 0);
    }
}

impl Drop for NativeRoot {
    fn drop(&mut self) {
        // All dependent wrappers release their lease after their SQLite call returns.
        let result = unsafe { domus_probe_root_drop(self.raw.as_ptr()) };
        self.release.set(if result == 0 {
            Release::Complete
        } else {
            eprintln!("native storage context quarantined after SQLite close error {result}");
            Release::Quarantined(result)
        });
    }
}

struct Connection {
    raw: Option<NonNull<Database>>,
    root: Rc<NativeRoot>,
}

impl Connection {
    fn open(root: &Rc<NativeRoot>) -> Self {
        let raw = unsafe { domus_probe_connection(root.raw.as_ptr()) };
        let raw = NonNull::new(raw).expect("real SQLite connection");
        unsafe { domus_probe_seed(raw.as_ptr()) };
        Self {
            raw: Some(raw),
            root: Rc::clone(root),
        }
    }

    fn raw(&self) -> *mut Database {
        self.raw.expect("live connection").as_ptr()
    }

    fn query(&self) -> Statement {
        Statement {
            raw: prepare(self.raw(), c"SELECT id,payload FROM temp.spill ORDER BY id"),
            root: Rc::clone(&self.root),
        }
    }

    fn blob(&self) -> Blob {
        let mut raw = ptr::null_mut();
        assert_eq!(
            unsafe {
                sqlite3_blob_open(
                    self.raw(),
                    c"temp".as_ptr(),
                    c"spill".as_ptr(),
                    c"payload".as_ptr(),
                    1,
                    0,
                    &mut raw,
                )
            },
            0
        );
        Blob {
            raw: NonNull::new(raw).unwrap(),
            root: Rc::clone(&self.root),
        }
    }

    fn retire(mut self) {
        let raw = self.raw.take().unwrap();
        assert_eq!(unsafe { sqlite3_close_v2(raw.as_ptr()) }, 0);
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        if let Some(raw) = self.raw.take() {
            assert_eq!(unsafe { sqlite3_close_v2(raw.as_ptr()) }, 0);
        }
    }
}

fn prepare(db: *mut Database, sql: &CStr) -> NonNull<Prepared> {
    let mut statement = ptr::null_mut();
    assert_eq!(
        unsafe { sqlite3_prepare_v2(db, sql.as_ptr(), -1, &mut statement, ptr::null_mut()) },
        0
    );
    NonNull::new(statement).expect("prepared statement")
}

fn verify_rows(statement: NonNull<Prepared>) {
    for key in 1..=8192 {
        assert_eq!(unsafe { sqlite3_step(statement.as_ptr()) }, 100);
        assert_eq!(unsafe { sqlite3_column_int(statement.as_ptr(), 0) }, key);
        assert_eq!(unsafe { sqlite3_column_bytes(statement.as_ptr(), 1) }, 1032);
        let text = unsafe { sqlite3_column_text(statement.as_ptr(), 1) };
        assert!(!text.is_null());
        let actual = unsafe { std::slice::from_raw_parts(text, 1032) };
        let expected = format!("{:08}{}", 8193 - key, "0".repeat(1024));
        assert_eq!(actual, expected.as_bytes());
    }
    assert_eq!(unsafe { sqlite3_step(statement.as_ptr()) }, 101);
}

struct Statement {
    raw: NonNull<Prepared>,
    root: Rc<NativeRoot>,
}

impl Drop for Statement {
    fn drop(&mut self) {
        self.root.assert_live();
        assert_eq!(unsafe { sqlite3_finalize(self.raw.as_ptr()) }, 0);
    }
}

struct Blob {
    raw: NonNull<NativeBlob>,
    root: Rc<NativeRoot>,
}

impl Drop for Blob {
    fn drop(&mut self) {
        self.root.assert_live();
        assert_eq!(unsafe { sqlite3_blob_close(self.raw.as_ptr()) }, 0);
    }
}

struct Memory(NonNull<Database>);

impl Memory {
    fn new() -> Self {
        let mut raw = ptr::null_mut();
        assert_eq!(unsafe { sqlite3_open(c":memory:".as_ptr(), &mut raw) }, 0);
        Self(NonNull::new(raw).unwrap())
    }
}

impl Drop for Memory {
    fn drop(&mut self) {
        assert_eq!(unsafe { sqlite3_close(self.0.as_ptr()) }, 0);
    }
}

struct Backup<'a> {
    raw: NonNull<NativeBackup>,
    root: Rc<NativeRoot>,
    _destination: &'a Memory,
}

impl<'a> Backup<'a> {
    fn start(source: &Connection, destination: &'a Memory) -> Self {
        let raw = unsafe {
            sqlite3_backup_init(
                destination.0.as_ptr(),
                c"main".as_ptr(),
                source.raw(),
                c"temp".as_ptr(),
            )
        };
        let raw = NonNull::new(raw).expect("real native TEMP backup");
        assert_eq!(unsafe { sqlite3_backup_step(raw.as_ptr(), 1) }, 0);
        assert!(unsafe { sqlite3_backup_remaining(raw.as_ptr()) } > 0);
        Self {
            raw,
            root: Rc::clone(&source.root),
            _destination: destination,
        }
    }
}

impl Drop for Backup<'_> {
    fn drop(&mut self) {
        self.root.assert_live();
        assert_eq!(unsafe { sqlite3_backup_finish(self.raw.as_ptr()) }, 0);
    }
}

#[cfg(windows)]
fn assert_pinned(path: &Path) {
    let error = std::fs::rename(path, path.with_extension("released"))
        .expect_err("actual retained native directory pin must deny replacement");
    assert!(matches!(error.raw_os_error(), Some(5 | 32)));
}

#[cfg(unix)]
fn assert_pinned(_path: &Path) {
    assert_eq!(unsafe { domus_probe_pins_live() }, 1);
}

fn assert_released(path: &Path, released: i32) {
    assert_eq!(unsafe { domus_probe_roots_released() }, released);
    assert_eq!(unsafe { domus_probe_temporary_live() }, 0);
    #[cfg(unix)]
    assert_eq!(unsafe { domus_probe_pins_closed() }, 1);
    std::fs::rename(path, path.with_extension("released"))
        .expect("all SQLite and directory handles released");
}

fn owner_thread(fixture: &Path) {
    let path = fixture.join("owner-thread");
    let (sender, receiver) = mpsc::sync_channel::<mpsc::SyncSender<usize>>(1);
    let caller_thread = std::thread::current().id();
    let worker = std::thread::spawn(move || {
        assert_ne!(std::thread::current().id(), caller_thread);
        let root = NativeRoot::bind(&path);
        let weak = Rc::downgrade(&root);
        let connection = Connection::open(&root);
        drop(root);
        let mut completed = 0;
        for reply in receiver {
            let query = connection.query();
            query.root.assert_live();
            assert_pinned(&path);
            verify_rows(query.raw);
            drop(query);
            completed += 1;
            reply.send(8192).unwrap();
        }
        assert_eq!(completed, 2);
        connection.retire();
        assert!(weak.upgrade().is_none());
        assert_released(&path, 4);
    });
    let barrier = Arc::new(Barrier::new(3));
    let clients: Vec<_> = (0..2)
        .map(|_| {
            let sender = sender.clone();
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                let (reply, result) = mpsc::sync_channel(0);
                barrier.wait();
                sender.send(reply).unwrap();
                assert_eq!(result.recv().unwrap(), 8192);
            })
        })
        .collect();
    drop(sender);
    barrier.wait();
    for client in clients {
        client.join().unwrap();
    }
    worker.join().unwrap();
}

fn native_close_failure(fixture: &Path) {
    let path = fixture.join("native-close-failure");
    let root = NativeRoot::bind(&path);
    let release = Rc::clone(&root.release);
    let connection = Connection::open(&root);
    unsafe { domus_probe_protect_temporary(connection.raw()) };
    drop(root);
    connection.retire();
    let sqlite_ioerr_close = 10 | (16 << 8);
    assert_eq!(release.get(), Release::Quarantined(sqlite_ioerr_close));
    assert_eq!(unsafe { domus_probe_roots_released() }, 4);
    assert_eq!(
        unsafe { domus_probe_verify_quarantine() },
        sqlite_ioerr_close
    );
    assert_pinned(&path);
    unsafe { domus_probe_recover_fixture() };
    assert_released(&path, 5);
    assert_eq!(release.get(), Release::Quarantined(sqlite_ioerr_close));
}

#[test]
fn sqlite_dependents_own_actual_native_context() {
    let fixture = std::env::var_os("DOMUS_SQLITE_OWNERSHIP_ROOT").expect("fresh fixture selection");
    let fixture = Path::new(&fixture);

    let path = fixture.join("statement");
    let root = NativeRoot::bind(&path);
    let weak = Rc::downgrade(&root);
    let connection = Connection::open(&root);
    let statement = connection.query();
    drop(root);
    connection.retire();
    statement.root.assert_live();
    assert_pinned(&path);
    verify_rows(statement.raw);
    drop(statement);
    assert!(weak.upgrade().is_none());
    assert_released(&path, 1);

    let path = fixture.join("blob");
    let root = NativeRoot::bind(&path);
    let weak = Rc::downgrade(&root);
    let connection = Connection::open(&root);
    let blob = connection.blob();
    drop(root);
    connection.retire();
    blob.root.assert_live();
    assert_pinned(&path);
    let mut bytes = [0u8; 1032];
    assert_eq!(
        unsafe { sqlite3_blob_read(blob.raw.as_ptr(), bytes.as_mut_ptr().cast(), 1032, 0) },
        0
    );
    assert_eq!(&bytes[..8], b"00008192");
    assert!(bytes[8..].iter().all(|byte| *byte == b'0'));
    drop(blob);
    assert!(weak.upgrade().is_none());
    assert_released(&path, 2);

    let path = fixture.join("backup");
    let root = NativeRoot::bind(&path);
    let weak = Rc::downgrade(&root);
    let connection = Connection::open(&root);
    let destination = Memory::new();
    let backup = Backup::start(&connection, &destination);
    drop(root);
    connection.retire();
    backup.root.assert_live();
    assert_pinned(&path);
    assert_eq!(unsafe { sqlite3_backup_step(backup.raw.as_ptr(), -1) }, 101);
    drop(backup);
    assert!(weak.upgrade().is_none());
    assert_released(&path, 3);
    let query = prepare(
        destination.0.as_ptr(),
        c"SELECT id,payload FROM main.spill ORDER BY id",
    );
    verify_rows(query);
    assert_eq!(unsafe { sqlite3_finalize(query.as_ptr()) }, 0);
    drop(destination);

    owner_thread(fixture);
    native_close_failure(fixture);

    let released = unsafe { domus_probe_roots_released() };
    let failed_close_calls = unsafe { domus_probe_native_close_errors() };
    assert!(failed_close_calls > 0);
    println!(
        "DOMUS_SQLITE_OWNERSHIP_EVIDENCE={{\"native_roots_released\":{released},\
        \"statement_rows_verified\":8192,\"backup_rows_verified\":8192,\
        \"blob_bytes_verified\":1032,\"connection_retired_before_dependent_work\":true,\
        \"native_pins_verified\":true,\"native_contexts_thread_confined\":true,\
        \"owner_thread_queue_capacity\":1,\"concurrent_client_requests_verified\":2,\
        \"owner_thread_rows_verified\":16384,\"native_close_failure\":{{\
        \"failed_files\":1,\"failed_native_calls\":{failed_close_calls},\
        \"context_quarantined\":true,\"admission_refused\":true,\
        \"explicit_fixture_recovery\":true}}}}"
    );
}
