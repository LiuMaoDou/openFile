//! Pause scanning at a read/commit boundary while a recycle batch changes files.
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Condvar, Mutex,
};

#[derive(Default)]
pub(crate) struct ScanGate {
    state: Mutex<State>,
    changed: Condvar,
    requested: AtomicBool,
}
#[derive(Default)]
struct State {
    paused: bool,
    readers: usize,
}
pub(crate) struct ScanPermit<'a>(&'a ScanGate);
pub(crate) struct ScanPause<'a>(&'a ScanGate);
impl ScanGate {
    pub fn is_paused(&self) -> bool {
        self.requested.load(Ordering::Acquire)
    }
    pub fn enter(&self) -> ScanPermit<'_> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        while state.paused {
            state = self.changed.wait(state).unwrap_or_else(|e| e.into_inner());
        }
        state.readers += 1;
        ScanPermit(self)
    }
    // The caller serializes recycle batches with file_operations.
    pub fn pause(&self) -> ScanPause<'_> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.paused = true;
        self.requested.store(true, Ordering::Release);
        while state.readers != 0 {
            state = self.changed.wait(state).unwrap_or_else(|e| e.into_inner());
        }
        ScanPause(self)
    }
}
impl Drop for ScanPermit<'_> {
    fn drop(&mut self) {
        self.0
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .readers -= 1;
        self.0.changed.notify_all();
    }
}
impl Drop for ScanPause<'_> {
    fn drop(&mut self) {
        let mut state = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
        state.paused = false;
        self.0.requested.store(false, Ordering::Release);
        self.0.changed.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        sync::{mpsc, Arc},
        thread,
        time::Duration,
    };
    #[test]
    fn pause_drains_observations_and_blocks_new_reads_until_resume() {
        let gate = Arc::new(ScanGate::default());
        let old_observation = gate.enter();
        let (paused_tx, paused_rx) = mpsc::channel();
        let (resume_tx, resume_rx) = mpsc::channel();
        let writer_gate = gate.clone();
        let writer = thread::spawn(move || {
            let _pause = writer_gate.pause();
            paused_tx.send(()).unwrap();
            resume_rx.recv().unwrap();
        });
        let start = std::time::Instant::now();
        while !gate.is_paused() {
            assert!(start.elapsed() < Duration::from_secs(3));
            thread::yield_now();
        }
        assert!(paused_rx.try_recv().is_err());
        drop(old_observation);
        paused_rx.recv_timeout(Duration::from_secs(3)).unwrap();
        let (read_tx, read_rx) = mpsc::channel();
        let reader_gate = gate.clone();
        let reader = thread::spawn(move || {
            let _permit = reader_gate.enter();
            read_tx.send(()).unwrap();
        });
        assert!(read_rx.recv_timeout(Duration::from_millis(50)).is_err());
        resume_tx.send(()).unwrap();
        read_rx.recv_timeout(Duration::from_secs(3)).unwrap();
        writer.join().unwrap();
        reader.join().unwrap();
        assert!(!gate.is_paused());
    }
}
