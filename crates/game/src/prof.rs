//! Temporary tick profiler (feature "profile"). It can be switched off at
//! run time (`set_enabled`), so a build that ships with it (the 3DS one,
//! for the SELECT overlay) only pays for the clock reads while it's shown.
#[cfg(feature = "profile")]
mod imp {
    extern crate std;
    use core::sync::atomic::{AtomicBool, Ordering};
    use std::cell::RefCell;
    use std::string::String;
    use std::time::Instant;
    use std::vec::Vec;
    std::thread_local! {
        // This profiler's bookkeeping predates const thread-locals and the
        // lint fires only in `--features profile` builds; not worth churning.
        #[allow(clippy::type_complexity, clippy::missing_const_for_thread_local)]
        static S: RefCell<(Option<Instant>, Vec<(String, f64)>)> = RefCell::new((None, Vec::new()));
    }
    static ENABLED: AtomicBool = AtomicBool::new(true);
    /// Records marks from now on (`true`, the default) or skips them.
    pub fn set_enabled(on: bool) {
        ENABLED.store(on, Ordering::Relaxed);
    }
    fn enabled() -> bool {
        ENABLED.load(Ordering::Relaxed)
    }
    pub fn start() {
        if !enabled() {
            return;
        }
        S.with(|s| s.borrow_mut().0 = Some(Instant::now()));
    }
    pub fn mark(name: &str) {
        if !enabled() {
            return;
        }
        S.with(|s| {
            let mut s = s.borrow_mut();
            let now = Instant::now();
            let dt = s.0.map_or(0.0, |t| (now - t).as_secs_f64());
            s.0 = Some(now);
            match s.1.iter_mut().find(|e| e.0 == name) {
                Some(e) => e.1 += dt,
                None => s.1.push((name.into(), dt)),
            }
        });
    }
    pub fn report() -> Vec<(String, f64)> {
        S.with(|s| s.borrow().1.clone())
    }
}
#[cfg(feature = "profile")]
pub use imp::*;
#[cfg(not(feature = "profile"))]
pub fn set_enabled(_: bool) {}
#[cfg(not(feature = "profile"))]
pub fn start() {}
#[cfg(not(feature = "profile"))]
pub fn mark(_: &str) {}
/// Nothing is recorded without the feature.
#[cfg(not(feature = "profile"))]
pub fn report() -> alloc::vec::Vec<(alloc::string::String, f64)> {
    alloc::vec::Vec::new()
}
