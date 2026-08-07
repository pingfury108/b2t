pub mod edge;

use std::sync::atomic::{AtomicUsize, Ordering};

/// 合成进度（供 worker 轮询写库）
#[derive(Default)]
pub struct Progress {
    pub done: AtomicUsize,
    pub total: AtomicUsize,
}

impl Progress {
    pub fn new(total: usize) -> Self {
        Self {
            done: AtomicUsize::new(0),
            total: AtomicUsize::new(total),
        }
    }
    pub fn incr(&self) {
        self.done.fetch_add(1, Ordering::Relaxed);
    }
    pub fn snapshot(&self) -> (usize, usize) {
        (
            self.done.load(Ordering::Relaxed),
            self.total.load(Ordering::Relaxed),
        )
    }
}
