//! Where the one event loop lives (ADR-324): a dedicated `brood-gui` thread where the
//! platform allows it, otherwise the process main thread, which `host_main_thread` keeps
//! parked for it while the runtime runs elsewhere; `start_thread` brings the loop up there.

use super::*;

/// Can the event loop live on a thread we choose, or must it own the **process main
/// thread**?
///
/// Wayland/X11 let a loop run anywhere, so there the GUI is a dedicated `brood-gui`
/// thread. macOS does not: AppKit's run loop is main-thread-only and winit exposes no
/// `with_any_thread` there (KI-125). Windows is the same shape. Anything not
/// on the permissive list therefore hosts the loop on the main thread.
const DEDICATED_THREAD_OK: bool = cfg!(any(
    target_os = "linux",
    target_os = "dragonfly",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd"
));

/// `BROOD_GUI_MAIN_THREAD=1` forces the main-thread path on a platform that does not
/// need it. It exists for one reason and it is not configuration: the main-thread path
/// is the ONLY path macOS can take, and there is no macOS in this project's CI beyond a
/// compile check — so without a lever it would ship having never once been run. With it
/// the identical code is exercised on Linux, where the GUI is actually tested.
fn main_thread_hosting_required() -> bool {
    static R: OnceLock<bool> = OnceLock::new();
    *R.get_or_init(|| {
        hosting_required_from(
            DEDICATED_THREAD_OK,
            std::env::var("BROOD_GUI_MAIN_THREAD").ok().as_deref(),
        )
    })
}

/// The decision itself, kept free of `cfg!`, the environment and the cache above so it
/// can be tested for every platform from whichever one the tests happen to run on —
/// which matters here more than usual, since the case that motivated this code
/// (`dedicated_ok == false`) is a platform CI only ever *compiles*.
pub(super) fn hosting_required_from(dedicated_ok: bool, lever: Option<&str>) -> bool {
    if !dedicated_ok {
        // Not a preference. macOS/Windows have nowhere else to put the loop, so the
        // lever cannot switch this off — `BROOD_GUI_MAIN_THREAD=0` there would only
        // turn a working GUI into one that fails to start.
        return true;
    }
    matches!(lever, Some(v) if v != "0" && !v.is_empty())
}

/// What the process main thread can be asked to do while the runtime runs elsewhere.
enum MainMsg {
    /// Build and run the one event loop *here*, replying with its proxy. Never returns:
    /// winit's `run_app` owns the thread from then on.
    HostGui(Sender<Result<EventLoopProxy<UserEvent>, String>>),
    /// The runtime thread finished without ever wanting a window; return normally.
    RuntimeDone,
}

/// The channel `host_main_thread` is listening on, published so `start_thread` can
/// reach it. `None` until a binary reserves its main thread — a library embedding the
/// runtime need not, and gets a clear error rather than a deadlock.
fn main_slot() -> &'static Mutex<Option<Sender<MainMsg>>> {
    static S: OnceLock<Mutex<Option<Sender<MainMsg>>>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(None))
}

/// Set once the main thread has been handed to winit, so the joiner below knows a
/// returning runtime can no longer be reported back through it.
static GUI_OWNS_MAIN: AtomicBool = AtomicBool::new(false);

const MAIN_THREAD_UNRESERVED: &str = "gui: this platform requires the event loop on the process main thread, and this \
     binary did not reserve it. A host embedding the brood runtime must run its work \
     through `cli_support::run_on_main_stack`, which parks the main thread for exactly \
     this.";

/// Run the runtime to completion while keeping the **process main thread** available
/// for a GUI event loop, and return whatever the runtime returned.
///
/// `handle` is the already-spawned runtime thread (`cli_support::run_on_main_stack`
/// sized it; that is the whole reason the runtime is not on the main thread in the
/// first place). On a platform where a dedicated GUI thread is fine this is exactly the
/// `join()` it replaces. Where it is not, this parks here instead: a small joiner
/// thread waits on the runtime, and the main thread blocks until either the runtime
/// finishes (return normally) or a `gui-open` asks for the loop — in which case winit
/// takes this thread for the life of the process.
///
/// The process then ends the way it always did, through the `std::process::exit` the
/// runtime's own exit path calls. The one case needing help is a runtime that *returns*
/// while winit owns this thread: nothing would notice, so the joiner exits the process
/// as a returning `main` would have.
pub fn host_main_thread<T: Send + 'static>(handle: JoinHandle<T>, name: &str) -> T {
    if !main_thread_hosting_required() {
        return handle
            .join()
            .unwrap_or_else(|_| panic!("{name} thread panicked"));
    }
    let (tx, rx) = mpsc::channel::<MainMsg>();
    *lock(main_slot()) = Some(tx.clone());

    let result: Arc<Mutex<Option<std::thread::Result<T>>>> = Arc::new(Mutex::new(None));
    let sink = Arc::clone(&result);
    let joined = std::thread::Builder::new()
        .name("brood-main-join".into())
        .spawn(move || {
            let r = handle.join();
            *lock(&sink) = Some(r);
            if GUI_OWNS_MAIN.load(Ordering::SeqCst) {
                // winit is never giving this thread back, so a returning runtime has to
                // end the process itself. Exit 0 is what a `main` that returned would
                // have produced; every non-zero path already went through
                // `std::process::exit` before reaching here.
                std::process::exit(0);
            }
            let _ = tx.send(MainMsg::RuntimeDone);
        });
    if let Err(e) = joined {
        panic!("spawn brood-main-join thread: {e}");
    }

    loop {
        match rx.recv() {
            Ok(MainMsg::HostGui(ready)) => run_gui(ready),
            Ok(MainMsg::RuntimeDone) | Err(_) => break,
        }
    }
    let outcome = result
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .take()
        .expect("runtime finished without recording a result");
    outcome.unwrap_or_else(|_| panic!("{name} thread panicked"))
}

/// Spawn the GUI thread + build the (single) event loop; return a proxy to it.
pub(super) fn start_thread() -> Result<EventLoopProxy<UserEvent>, String> {
    let (ready_tx, ready_rx) = mpsc::channel::<Result<EventLoopProxy<UserEvent>, String>>();
    if main_thread_hosting_required() {
        // Hand the loop to the parked main thread rather than spawning one. Safe to set
        // the flag after the send succeeds and before awaiting the proxy: the caller is
        // blocked in `recv` below, so the runtime cannot finish in the window between.
        let tx = main_slot()
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
            .ok_or_else(|| MAIN_THREAD_UNRESERVED.to_string())?;
        tx.send(MainMsg::HostGui(ready_tx))
            .map_err(|_| "gui: the process main thread is gone".to_string())?;
        GUI_OWNS_MAIN.store(true, Ordering::SeqCst);
    } else {
        std::thread::Builder::new()
            .name("brood-gui".into())
            .spawn(move || run_gui(ready_tx))
            .map_err(|e| e.to_string())?;
    }
    ready_rx
        .recv()
        .map_err(|_| "gui thread exited during init".to_string())?
}
