//! Handle-scoped SQLite with upstream native locking.
//!
//! The caller verifies the configured expectation, private permissions and local
//! filesystem before construction. Pass the actual no-follow directory chain,
//! not handles reconstructed from marker text. The adapter duplicates the pins
//! and checks every component and `sqlite-tmp` against its retained identity
//! before new I/O. Labels used by SQLite are routing tokens, never OS paths.
//!
//! Native descriptors and a SQLite connection destructor retain authority
//! independently of Rust owners. This includes upstream deferred descriptors
//! and `close_v2` dependents. Registry locks cover bookkeeping only. Native close
//! failure permanently quarantines the affected context and its evidence.
//! Recovery requires stopping the process and explicit operator action.
//!
//! The wrapper does not run schema SQL or choose journal/transaction policy.
//! Callers keep their existing busy timeout, foreign keys, WAL, FULL sync and
//! snapshot setup. Safe borrows preserve normal rusqlite statements,
//! transactions, blobs and backups. There is no consuming bare-connection escape.
//! Named ATTACH is refused. Empty ephemeral databases (including VACUUM's)
//! use the same private TEMP boundary, not another database stem.
//! Raw SQLite APIs remain unsafe: callers must not replace the authority
//! function, authorizer, VFS or native syscall implementation.

use std::ffi::{CString, OsString, c_char, c_int, c_void};
use std::fs::File;
use std::ops::Deref;
use std::sync::Arc;

pub use rusqlite;

mod selection;
pub use selection::{SELECTION_FILE, SelectionFence};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid native SQLite directory chain or database component")]
    InvalidInput,
    #[error("native SQLite namespace is stale or refused (SQLite code {0})")]
    Native(c_int),
    #[error("other Rust owners still retain the native SQLite context")]
    InUse,
    #[error("another owner retains a different frozen storage selection; stop all state users")]
    SelectionInUse,
    #[error("storage selection record is malformed; preserve it for explicit recovery")]
    SelectionRecord,
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

/// Retained native ancestry ending at the selected state directory.
///
/// `anchor` is the caller's trusted native namespace anchor. Each pair is one
/// direct component and its actual no-follow File pin. Include every intervening
/// ancestor. An empty component list means the anchor itself is state.
pub struct DirectoryChain {
    anchor: File,
    components: Vec<(OsString, File)>,
}

impl DirectoryChain {
    pub fn new(anchor: File, components: Vec<(OsString, File)>) -> Result<Self> {
        if components.len() >= 256 {
            return Err(Error::InvalidInput);
        }
        for (name, _) in &components {
            component(name.to_str().ok_or(Error::InvalidInput)?)?;
        }
        Ok(Self { anchor, components })
    }
}

/// Native SQLite open policy. Does not initialize the application schema.
#[derive(Clone, Copy, Debug)]
pub enum OpenMode {
    Create,
    ReadWrite,
    ReadOnly,
}

#[derive(Clone)]
pub struct Context(Arc<Inner>);

// An integer is an opaque registry handle, not a dereferenceable Rust pointer.
// C owns the allocation; its registry protects reference/admission bookkeeping.
// No Connection or borrowed SQL object is shared through this type.
struct Inner {
    handle: usize,
}

impl Drop for Inner {
    fn drop(&mut self) {
        // SAFETY: this Arc owns exactly one C context reference.
        // C retains failed-close evidence; Drop cannot return its error.
        if self.handle != 0 {
            let code = unsafe { domus_sqlite_release(self.handle as *mut c_void) };
            if code != rusqlite::ffi::SQLITE_OK {
                eprintln!(
                    "native SQLite authority release failed (SQLite code {code}); inspect quarantined state before recovery"
                );
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Statistics {
    pub temporary_opens: u64,
    pub temporary_bytes_written: u64,
    pub live_native_handles: u64,
    pub quarantined: bool,
}

impl Context {
    /// Duplicates retained pins after checking native object identities.
    ///
    /// `temporary` must be the existing private `sqlite-tmp` direct child of
    /// state. Creation and permission policy belong to the parent filesystem
    /// client. Linux additionally checks current-user ownership and mode 0700
    /// on the actual temporary directory at each native admission.
    /// Independent contexts for the same chain, temporary child and
    /// database stem share a native admission context.
    pub fn new(chain: DirectoryChain, temporary: File, database_leaf: &str) -> Result<Self> {
        Self::construct(chain, temporary, database_leaf, None)
    }

    /// Keep the original selection lock through every native dependent and
    /// close uncertainty. Its file must be the fixed direct child of state.
    pub fn new_fenced(
        chain: DirectoryChain,
        temporary: File,
        database_leaf: &str,
        selection: SelectionFence,
    ) -> Result<Self> {
        Self::construct(chain, temporary, database_leaf, Some(selection))
    }

    fn construct(
        chain: DirectoryChain,
        temporary: File,
        database_leaf: &str,
        selection: Option<SelectionFence>,
    ) -> Result<Self> {
        if database_leaf.eq_ignore_ascii_case(SELECTION_FILE) {
            return Err(Error::InvalidInput);
        }
        let leaf = component(database_leaf)?;
        let mut handles = Vec::with_capacity(chain.components.len() + 1);
        let mut names = Vec::with_capacity(chain.components.len() + 1);
        handles.push(raw(&chain.anchor));
        names.push(CString::default());
        for (name, file) in &chain.components {
            handles.push(raw(file));
            names.push(component(name.to_str().ok_or(Error::InvalidInput)?)?);
        }
        let name_pointers: Vec<_> = names.iter().map(|name| name.as_ptr()).collect();
        let mut output = std::ptr::null_mut();
        let mut adopted = 0;
        // SAFETY: all pointers are valid during this call. C duplicates directory
        // pins and reports ownership of the original selection handle even on
        // failure. It retains no Rust allocation or borrowed pointer.
        let code = unsafe {
            domus_sqlite_context(
                handles.as_ptr(),
                name_pointers.as_ptr(),
                handles.len() as c_int,
                raw(&temporary),
                leaf.as_ptr(),
                selection
                    .as_ref()
                    .map_or(-1, |selection| raw(&selection.file)),
                selection
                    .as_ref()
                    .map_or(std::ptr::null(), |selection| selection.record.as_ptr()),
                selection
                    .as_ref()
                    .map_or(0, |selection| selection.record.len() as c_int),
                &mut adopted,
                &mut output,
            )
        };
        if adopted != 0
            && let Some(selection) = selection
        {
            transfer(selection.file);
        }
        native_result(code)?;
        if output.is_null() {
            return Err(Error::Native(rusqlite::ffi::SQLITE_NOMEM));
        }
        Ok(Self(Arc::new(Inner {
            handle: output as usize,
        })))
    }

    pub fn check(&self) -> Result<()> {
        // SAFETY: the Arc keeps this C reference live.
        native_result(unsafe { domus_sqlite_check(self.pointer()) })
    }

    pub fn statistics(&self) -> Statistics {
        let mut values = [0_u64; 4];
        // SAFETY: C writes exactly four u64 values while the context is live.
        unsafe { domus_sqlite_stats(self.pointer(), values.as_mut_ptr()) };
        Statistics {
            temporary_opens: values[0],
            temporary_bytes_written: values[1],
            live_native_handles: values[2],
            quarantined: values[3] != 0,
        }
    }

    /// Releases the last Rust context owner and reports terminal pin-close
    /// failure. Raw SQLite dependents can still defer final native disposal.
    /// Ordinary Drop also retains failed-close evidence, but cannot return it.
    pub fn close(self) -> Result<()> {
        let mut inner = Arc::try_unwrap(self.0).map_err(|_| Error::InUse)?;
        let handle = std::mem::take(&mut inner.handle);
        // SAFETY: this is the unique Rust-owned C context reference.
        native_result(unsafe { domus_sqlite_release(handle as *mut c_void) })
    }

    pub fn open(&self, mode: OpenMode) -> Result<ScopedConnection> {
        self.check()?;
        // SAFETY: these immutable nul-terminated labels live with this context.
        let (vfs, leaf) = unsafe {
            (
                std::ffi::CStr::from_ptr(domus_sqlite_vfs(self.pointer())),
                std::ffi::CStr::from_ptr(domus_sqlite_leaf(self.pointer())),
            )
        };
        let flags = match mode {
            OpenMode::Create => {
                rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE
                    | rusqlite::OpenFlags::SQLITE_OPEN_CREATE
            }
            OpenMode::ReadWrite => rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE,
            OpenMode::ReadOnly => rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        } | rusqlite::OpenFlags::SQLITE_OPEN_NOFOLLOW
            | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX;
        let mut raw = std::ptr::null_mut();
        // SAFETY: both C strings and the output are valid for this call.
        let code = unsafe {
            rusqlite::ffi::sqlite3_open_v2(leaf.as_ptr(), &mut raw, flags.bits(), vfs.as_ptr())
        };
        if code != rusqlite::ffi::SQLITE_OK {
            if !raw.is_null() {
                // SAFETY: failed SQLite opens still return an owned handle.
                unsafe { rusqlite::ffi::sqlite3_close(raw) };
            }
            return Err(Error::Native(code));
        }
        // SAFETY: the C destructor adds an independent context reference.
        let code = unsafe { domus_sqlite_bind(self.pointer(), raw) };
        if code != rusqlite::ffi::SQLITE_OK {
            // SAFETY: no dependents exist on this new connection.
            unsafe { rusqlite::ffi::sqlite3_close(raw) };
            return Err(Error::Native(code));
        }
        // SAFETY: ScopedConnection owns raw and destroys this non-owning facade
        // before closing it. No mutable facade escape is exposed.
        let connection = match unsafe { rusqlite::Connection::from_handle(raw) } {
            Ok(connection) => connection,
            Err(error) => {
                // SAFETY: this new connection has no dependents.
                unsafe { rusqlite::ffi::sqlite3_close(raw) };
                return Err(Error::Sqlite(error));
            }
        };
        let scoped = ScopedConnection {
            connection: Some(connection),
            context: self.clone(),
            raw: raw as usize,
        };
        scoped.busy_timeout(std::time::Duration::from_secs(5))?;
        Ok(scoped)
    }

    fn pointer(&self) -> *mut c_void {
        self.0.handle as *mut c_void
    }
}

/// Owns the connection before its context, so normal Drop closes SQLite first.
pub struct ScopedConnection {
    connection: Option<rusqlite::Connection>,
    context: Context,
    raw: usize,
}

impl ScopedConnection {
    /// Reports native close failure even when upstream SQLite reports success.
    pub fn close(mut self) -> Result<()> {
        drop(self.connection.take());
        let raw = std::mem::take(&mut self.raw) as *mut rusqlite::ffi::sqlite3;
        // SAFETY: the non-owning facade is gone. This object owns raw.
        let code = unsafe { rusqlite::ffi::sqlite3_close(raw) };
        if code != rusqlite::ffi::SQLITE_OK {
            // SAFETY: close failed, so raw is still live. The lifetime token
            // and native descriptor refs survive close_v2 dependents.
            unsafe { sqlite3_close_v2(raw) };
            self.context.check()?;
            return Err(Error::Native(code));
        }
        self.context.check()
    }

    pub fn context(&self) -> &Context {
        &self.context
    }

    pub fn transaction(&mut self) -> rusqlite::Result<rusqlite::Transaction<'_>> {
        self.transaction_with_behavior(rusqlite::TransactionBehavior::Deferred)
    }

    pub fn transaction_with_behavior(
        &mut self,
        behavior: rusqlite::TransactionBehavior,
    ) -> rusqlite::Result<rusqlite::Transaction<'_>> {
        match self.connection.as_mut() {
            Some(connection) => connection.transaction_with_behavior(behavior),
            None => unreachable!("a consumed scoped connection cannot be borrowed"),
        }
    }
}

impl Drop for ScopedConnection {
    fn drop(&mut self) {
        drop(self.connection.take());
        if self.raw != 0 {
            // SAFETY: the non-owning facade is gone. close_v2 defers disposal
            // for any outstanding raw dependent. C retains its own authority.
            let code = unsafe { sqlite3_close_v2(self.raw as *mut rusqlite::ffi::sqlite3) };
            self.raw = 0;
            if code != rusqlite::ffi::SQLITE_OK || self.context.statistics().quarantined {
                eprintln!(
                    "native SQLite connection close failed (SQLite code {code}); inspect retained state before recovery"
                );
            }
        }
    }
}

impl Deref for ScopedConnection {
    type Target = rusqlite::Connection;

    fn deref(&self) -> &Self::Target {
        match &self.connection {
            Some(connection) => connection,
            None => unreachable!("a consumed scoped connection cannot be borrowed"),
        }
    }
}

fn component(value: &str) -> Result<CString> {
    if value.is_empty()
        || value.len() > 255
        || value == "."
        || value == ".."
        || value.ends_with(['.', ' '])
        || value
            .chars()
            .any(|c| c.is_control() || "/\\:*?\"<>|".contains(c))
    {
        return Err(Error::InvalidInput);
    }
    CString::new(value).map_err(|_| Error::InvalidInput)
}

fn native_result(code: c_int) -> Result<()> {
    if code == rusqlite::ffi::SQLITE_OK {
        Ok(())
    } else {
        Err(Error::Native(code))
    }
}

#[cfg(unix)]
fn raw(file: &File) -> isize {
    use std::os::fd::AsRawFd;
    file.as_raw_fd() as isize
}

fn transfer(file: File) {
    #[cfg(unix)]
    {
        use std::os::fd::IntoRawFd;
        let _ = file.into_raw_fd();
    }
    #[cfg(windows)]
    {
        use std::os::windows::io::IntoRawHandle;
        let _ = file.into_raw_handle();
    }
}

#[cfg(windows)]
fn raw(file: &File) -> isize {
    use std::os::windows::io::AsRawHandle;
    file.as_raw_handle() as isize
}

unsafe extern "C" {
    fn sqlite3_close_v2(connection: *mut rusqlite::ffi::sqlite3) -> c_int;
    fn domus_sqlite_context(
        handles: *const isize,
        names: *const *const c_char,
        count: c_int,
        temporary: isize,
        leaf: *const c_char,
        selection: isize,
        selection_record: *const u8,
        selection_size: c_int,
        selection_adopted: *mut c_int,
        output: *mut *mut c_void,
    ) -> c_int;
    fn domus_sqlite_check(context: *mut c_void) -> c_int;
    fn domus_sqlite_release(context: *mut c_void) -> c_int;
    fn domus_sqlite_vfs(context: *mut c_void) -> *const c_char;
    fn domus_sqlite_leaf(context: *mut c_void) -> *const c_char;
    fn domus_sqlite_bind(context: *mut c_void, connection: *mut rusqlite::ffi::sqlite3) -> c_int;
    fn domus_sqlite_stats(context: *mut c_void, out: *mut u64);
}
