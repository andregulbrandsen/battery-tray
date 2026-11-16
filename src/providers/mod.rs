pub mod bluetooth;
pub mod logitech;

use crate::model::Reading;

/// One full scan of every source. Runs on the worker thread.
pub fn poll_all() -> Vec<Reading> {
    let mut out = bluetooth::poll();
    out.extend(logitech::poll());
    out
}
