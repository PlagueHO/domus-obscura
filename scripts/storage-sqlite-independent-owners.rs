//! Bounded independent-owner proof, not a production registry or bootstrap API.
//! Rust shares an actual directory File, never a connection or a raw context pointer.

use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::fs::File;
use std::path::{Path, PathBuf};
use std::ptr::{self, NonNull};
use std::sync::{Arc, Barrier, Mutex, mpsc};

type Database = c_void;
type Prepared = c_void;

unsafe extern "C" {
    fn domus_probe_root_new(path: *const c_char) -> *mut c_void;
    fn domus_independent_pin() -> isize;
    fn domus_independent_live(pin: isize) -> c_int;
    fn domus_independent_pin_closed(pin: isize) -> c_int;
    fn domus_independent_connection(pin: isize) -> *mut Database;
    fn domus_independent_drop(pin: isize) -> c_int;
    fn domus_independent_calls() -> c_int;
    fn domus_independent_quarantine() -> c_int;
    fn domus_probe_roots_released() -> c_int;
    fn domus_probe_native_close_errors() -> c_int;
    fn domus_probe_protect_temporary(db: *mut Database);
    fn domus_probe_recover_fixture();
    fn sqlite3_exec(
        db: *mut Database,
        sql: *const c_char,
        callback: Option<
            unsafe extern "C" fn(*mut c_void, c_int, *mut *mut c_char, *mut *mut c_char) -> c_int,
        >,
        context: *mut c_void,
        error: *mut *mut c_char,
    ) -> c_int;
    fn sqlite3_errmsg(db: *mut Database) -> *const c_char;
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
    fn sqlite3_column_text(statement: *mut Prepared, column: c_int) -> *const c_char;
    fn sqlite3_column_bytes(statement: *mut Prepared, column: c_int) -> c_int;
    fn sqlite3_finalize(statement: *mut Prepared) -> c_int;
    fn sqlite3_create_function_v2(
        db: *mut Database,
        name: *const c_char,
        arguments: c_int,
        encoding: c_int,
        data: *mut c_void,
        scalar: Option<unsafe extern "C" fn(*mut c_void, c_int, *mut *mut c_void)>,
        step: Option<unsafe extern "C" fn(*mut c_void, c_int, *mut *mut c_void)>,
        finish: Option<unsafe extern "C" fn(*mut c_void)>,
        destroy: Option<unsafe extern "C" fn(*mut c_void)>,
    ) -> c_int;
    fn sqlite3_user_data(context: *mut c_void) -> *mut c_void;
    fn sqlite3_result_int(context: *mut c_void, value: c_int);
}

#[derive(Clone, PartialEq, Eq)]
struct Selection {
    revision: u64,
    path: PathBuf,
}

struct Authority {
    pin: File,
    expected: Selection,
    selected: Arc<Mutex<Selection>>,
    release: Arc<Mutex<Option<c_int>>>,
    invalid: Mutex<bool>,
}

impl Authority {
    fn bind(path: &Path) -> Arc<Self> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            std::fs::DirBuilder::new().mode(0o700).create(path).unwrap();
        }
        #[cfg(windows)]
        std::fs::create_dir(path).unwrap();
        let native_path = CString::new(path.to_str().unwrap()).unwrap();
        assert!(!unsafe { domus_probe_root_new(native_path.as_ptr()) }.is_null());
        let pin = unsafe { domus_independent_pin() };
        assert!(pin != -1 && pin != 0);
        #[cfg(unix)]
        let pin = {
            use std::os::fd::FromRawFd;
            unsafe { File::from_raw_fd(c_int::try_from(pin).unwrap()) }
        };
        #[cfg(windows)]
        let pin = {
            use std::os::windows::io::FromRawHandle;
            unsafe { File::from_raw_handle(pin as *mut c_void) }
        };
        let expected = Selection {
            revision: 1,
            path: path.to_owned(),
        };
        Arc::new(Self {
            pin,
            selected: Arc::new(Mutex::new(expected.clone())),
            expected,
            release: Arc::new(Mutex::new(None)),
            invalid: Mutex::new(false),
        })
    }

    fn native(&self) -> isize {
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            self.pin.as_raw_fd() as isize
        }
        #[cfg(windows)]
        {
            use std::os::windows::io::AsRawHandle;
            self.pin.as_raw_handle() as isize
        }
    }

    fn assert_live(&self) {
        assert!(self.pin.metadata().unwrap().is_dir());
        assert_eq!(unsafe { domus_independent_live(self.native()) }, 1);
    }

    fn connect(self: &Arc<Self>) -> Result<Connection, &'static str> {
        let selected = self.selected.lock().unwrap();
        let mut invalid = self.invalid.lock().unwrap();
        if *invalid || *selected != self.expected {
            *invalid = true;
            return Err("stale startup selection");
        }
        let raw = unsafe { domus_independent_connection(self.native()) };
        Ok(Connection {
            raw: Some(NonNull::new(raw).expect("real native SQLite connection")),
            root: Arc::clone(self),
        })
    }
}

impl Drop for Authority {
    fn drop(&mut self) {
        self.assert_live();
        let result = unsafe { domus_independent_drop(self.native()) };
        if result != 0 {
            eprintln!("shared native context quarantined after actual close failure: {result}");
        }
        *self.release.lock().unwrap() = Some(result);
    }
}

struct Connection {
    raw: Option<NonNull<Database>>,
    root: Arc<Authority>,
}

impl Connection {
    fn raw(&self) -> *mut Database {
        self.raw.unwrap().as_ptr()
    }

    fn execute(&self, sql: &str) {
        let sql = CString::new(sql).unwrap();
        let result = unsafe {
            sqlite3_exec(
                self.raw(),
                sql.as_ptr(),
                None,
                ptr::null_mut(),
                ptr::null_mut(),
            )
        };
        assert_eq!(result, 0, "{}", unsafe {
            CStr::from_ptr(sqlite3_errmsg(self.raw())).to_string_lossy()
        });
    }

    fn query(&self, sql: &CStr) -> Statement {
        let mut raw = ptr::null_mut();
        assert_eq!(
            unsafe { sqlite3_prepare_v2(self.raw(), sql.as_ptr(), -1, &mut raw, ptr::null_mut()) },
            0
        );
        Statement {
            raw: NonNull::new(raw).unwrap(),
            root: Arc::clone(&self.root),
        }
    }

    fn retire(mut self) {
        assert_eq!(
            unsafe { sqlite3_close_v2(self.raw.take().unwrap().as_ptr()) },
            0
        );
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        if let Some(raw) = self.raw.take() {
            assert_eq!(unsafe { sqlite3_close_v2(raw.as_ptr()) }, 0);
        }
    }
}

struct Statement {
    raw: NonNull<Prepared>,
    root: Arc<Authority>,
}

impl Statement {
    fn row(&self, key: i32, temporary: bool) {
        assert_eq!(unsafe { sqlite3_step(self.raw.as_ptr()) }, 100);
        assert_eq!(unsafe { sqlite3_column_int(self.raw.as_ptr(), 0) }, key);
        let expected = if temporary {
            format!("{:08}{}", 8193 - key, "0".repeat(1024))
        } else {
            format!("payload-{}", key * 17)
        };
        let text = unsafe { sqlite3_column_text(self.raw.as_ptr(), 1) };
        assert!(!text.is_null());
        assert_eq!(
            unsafe { sqlite3_column_bytes(self.raw.as_ptr(), 1) } as usize,
            expected.len()
        );
        assert_eq!(
            unsafe { CStr::from_ptr(text) }.to_bytes(),
            expected.as_bytes()
        );
    }

    fn done(&self) {
        assert_eq!(unsafe { sqlite3_step(self.raw.as_ptr()) }, 101);
    }
}

impl Drop for Statement {
    fn drop(&mut self) {
        self.root.assert_live();
        assert_eq!(unsafe { sqlite3_finalize(self.raw.as_ptr()) }, 0);
    }
}

unsafe extern "C" fn rendezvous(context: *mut c_void, _: c_int, _: *mut *mut c_void) {
    let barrier = unsafe { &*sqlite3_user_data(context).cast::<Barrier>() };
    barrier.wait();
    unsafe { sqlite3_result_int(context, 1) };
}

fn overlap(connection: &Connection, barrier: &Barrier) {
    assert_eq!(
        unsafe {
            sqlite3_create_function_v2(
                connection.raw(),
                c"rendezvous".as_ptr(),
                0,
                1,
                (barrier as *const Barrier).cast_mut().cast(),
                Some(rendezvous),
                None,
                None,
                None,
            )
        },
        0
    );
    connection.execute("SELECT rendezvous()");
    assert_eq!(
        unsafe {
            sqlite3_create_function_v2(
                connection.raw(),
                c"rendezvous".as_ptr(),
                0,
                1,
                ptr::null_mut(),
                None,
                None,
                None,
                None,
            )
        },
        0
    );
}

const TEMP: &str = "PRAGMA temp_store=FILE; PRAGMA temp.cache_size=8;\
    CREATE TEMP TABLE spill(id INTEGER PRIMARY KEY,payload TEXT);\
    WITH RECURSIVE source(x) AS(VALUES(1) UNION ALL SELECT x+1 FROM source WHERE x<8192)\
    INSERT INTO spill SELECT x,printf('%08d',8193-x)||hex(zeroblob(512)) FROM source;";

fn concurrent_owners(fixture: &Path) {
    fn send_sync<T: Send + Sync>() {}
    send_sync::<Authority>();
    let root = Authority::bind(&fixture.join("concurrent"));
    let seed = root.connect().unwrap();
    seed.execute(
        "PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;\
        CREATE TABLE entries(id INTEGER PRIMARY KEY,payload TEXT);\
        WITH RECURSIVE source(x) AS(VALUES(1) UNION ALL SELECT x+1 FROM source WHERE x<8192)\
        INSERT INTO entries SELECT x,printf('payload-%d',x*17) FROM source;",
    );
    drop(seed);
    let release = Arc::clone(&root.release);
    let barrier = Arc::new(Barrier::new(2));
    let (ready_tx, ready_rx) = mpsc::sync_channel(1);
    let (commit_tx, commit_rx) = mpsc::sync_channel(1);
    let reader = {
        let root = Arc::clone(&root);
        let barrier = Arc::clone(&barrier);
        std::thread::spawn(move || {
            let connection = root.connect().unwrap();
            connection.execute(TEMP);
            overlap(&connection, &barrier);
            connection.execute("BEGIN");
            let snapshot = connection.query(c"SELECT id,payload FROM entries ORDER BY id");
            snapshot.row(1, false);
            let temporary = connection.query(c"SELECT id,payload FROM temp.spill ORDER BY id");
            connection.retire();
            ready_tx.send(()).unwrap();
            commit_rx.recv().unwrap();
            root.assert_live();
            for key in 2..=8192 {
                snapshot.row(key, false);
            }
            snapshot.done();
            drop(snapshot);
            for key in 1..=8192 {
                temporary.row(key, true);
            }
            temporary.done();
            drop(temporary);
            (root, std::thread::current().id())
        })
    };
    let writer = {
        let root = Arc::clone(&root);
        std::thread::spawn(move || {
            let connection = root.connect().unwrap();
            connection.execute(TEMP);
            overlap(&connection, &barrier);
            ready_rx.recv().unwrap();
            connection.execute("BEGIN IMMEDIATE");
            for key in 8193..=8448 {
                connection.execute(&format!(
                    "INSERT INTO entries VALUES({key},'payload-{}')",
                    key * 17
                ));
            }
            connection.execute("COMMIT");
            commit_tx.send(()).unwrap();
            let current = connection.query(c"SELECT id,payload FROM entries ORDER BY id");
            for key in 1..=8448 {
                current.row(key, false);
            }
            current.done();
            drop(current);
            drop(connection);
            (root, std::thread::current().id())
        })
    };
    drop(root);
    assert_eq!(*release.lock().unwrap(), None);
    let (reader_root, reader_id) = reader.join().unwrap();
    let (writer_root, writer_id) = writer.join().unwrap();
    assert_ne!(reader_id, writer_id);
    assert_ne!(reader_id, std::thread::current().id());
    assert_ne!(writer_id, std::thread::current().id());
    reader_root.assert_live();
    assert_eq!(unsafe { domus_probe_native_close_errors() }, 0);
    let replacement = fixture.join("unadmitted");
    std::fs::create_dir(&replacement).unwrap();
    *reader_root.selected.lock().unwrap() = Selection {
        revision: 2,
        path: replacement.clone(),
    };
    let before = unsafe { domus_independent_calls() };
    assert!(matches!(
        reader_root.connect(),
        Err("stale startup selection")
    ));
    *reader_root.selected.lock().unwrap() = reader_root.expected.clone();
    assert!(matches!(
        writer_root.connect(),
        Err("stale startup selection")
    ));
    assert_eq!(unsafe { domus_independent_calls() }, before);
    assert_eq!(std::fs::read_dir(replacement).unwrap().count(), 0);
    drop(reader_root);
    assert_eq!(*release.lock().unwrap(), None);
    let pin = writer_root.native();
    drop(writer_root);
    assert_eq!(*release.lock().unwrap(), Some(0));
    assert_eq!(unsafe { domus_independent_pin_closed(pin) }, 1);
    assert_eq!(unsafe { domus_probe_roots_released() }, 1);
}

fn concurrent_quarantine(fixture: &Path) -> c_int {
    let root = Authority::bind(&fixture.join("fault"));
    let peer = root.connect().unwrap();
    peer.execute("PRAGMA journal_mode=WAL; CREATE TABLE preserved(value INTEGER); INSERT INTO preserved VALUES(41)");
    let preserved = peer.query(c"SELECT value FROM preserved");
    assert_eq!(unsafe { sqlite3_step(preserved.raw.as_ptr()) }, 100);
    assert_eq!(unsafe { sqlite3_column_int(preserved.raw.as_ptr(), 0) }, 41);
    let release = Arc::clone(&root.release);
    let fault = Arc::clone(&root);
    std::thread::spawn(move || {
        let connection = fault.connect().unwrap();
        connection.execute(TEMP);
        unsafe { domus_probe_protect_temporary(connection.raw()) };
        connection.retire();
    })
    .join()
    .unwrap();
    let failed_calls = unsafe { domus_probe_native_close_errors() };
    #[cfg(unix)]
    assert_eq!(failed_calls, 1);
    #[cfg(windows)]
    assert!(failed_calls > 0);
    assert_eq!(*release.lock().unwrap(), None);
    root.assert_live();
    assert_eq!(unsafe { domus_independent_quarantine() }, 4106);
    drop(root);
    peer.root.assert_live();
    assert_eq!(*release.lock().unwrap(), None);
    drop(peer);
    assert_eq!(*release.lock().unwrap(), None);
    preserved.root.assert_live();
    preserved.done();
    let pin = preserved.root.native();
    drop(preserved);
    assert_eq!(*release.lock().unwrap(), Some(4106));
    assert_eq!(unsafe { domus_independent_pin_closed(pin) }, 1);
    assert_eq!(unsafe { domus_probe_roots_released() }, 1);
    unsafe { domus_probe_recover_fixture() };
    assert_eq!(unsafe { domus_probe_roots_released() }, 2);
    failed_calls
}

#[test]
fn independent_connections_retain_native_authority() {
    let fixture = std::env::var_os("DOMUS_SQLITE_OWNERSHIP_ROOT").unwrap();
    let fixture = Path::new(&fixture);
    concurrent_owners(fixture);
    let failed_calls = concurrent_quarantine(fixture);
    println!(
        "DOMUS_SQLITE_INDEPENDENT_EVIDENCE={{\
        \"independent_connection_owners\":2,\"overlapping_sqlite_calls\":true,\
        \"snapshot_rows\":8192,\"committed_rows\":8448,\"temporary_rows\":8192,\
        \"deferred_close_verified\":true,\"stale_selection_refusals\":2,\
        \"native_close_failed_files\":1,\"native_close_failed_calls\":{failed_calls},\
        \"quarantine_with_live_peer\":true,\
        \"peer_receipt_value\":41,\"rust_directory_pins_closed\":2,\
        \"native_contexts_released\":2,\"production_registry\":false}}"
    );
}
