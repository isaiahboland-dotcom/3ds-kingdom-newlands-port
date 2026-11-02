//! Temporary tick profiler (feature "profile").
#[cfg(feature = "profile")]
mod imp {
    extern crate std;
    use std::cell::RefCell;
    use std::string::String;
    use std::time::Instant;
    use std::vec::Vec;
    std::thread_local! {
        static S: RefCell<(Option<Instant>, Vec<(String, f64)>)> = RefCell::new((None, Vec::new()));
    }
    pub fn start() {
        S.with(|s| s.borrow_mut().0 = Some(Instant::now()));
    }
    pub fn mark(name: &str) {
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
pub fn start() {}
#[cfg(not(feature = "profile"))]
pub fn mark(_: &str) {}
