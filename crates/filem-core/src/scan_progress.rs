//! Live scan telemetry is independent of SQLite, so a slow disk/write cannot hide it.
use crate::model::ScanRun;
use std::sync::{Arc, Mutex};
use std::time::Instant;

#[derive(Default)]
pub(crate) struct Registry {
    next: u64,
    runs: Vec<Arc<Mutex<Tracked>>>,
}
struct Tracked {
    started: Instant,
    advanced: Instant,
    run: ScanRun,
}
pub(crate) struct Run(Arc<Mutex<Tracked>>);

impl Registry {
    pub fn start(&mut self, scopes: Vec<String>, reason: &str) -> Run {
        // Retain completed members while other jobs in the same round are running.
        if self.runs.iter().all(|r| r.lock().unwrap().run.finished) {
            self.runs.clear();
        } else {
            // A blocked disk must not retain an unbounded history of rescans on
            // another disk, or count an earlier pass of the same scope twice.
            self.runs.retain(|r| {
                let run = &r.lock().unwrap().run;
                !run.finished || !run.scope_ids.iter().any(|id| scopes.contains(id))
            });
            while self.runs.len() >= 64 {
                let Some(index) = self
                    .runs
                    .iter()
                    .position(|r| r.lock().unwrap().run.finished)
                else {
                    break;
                };
                self.runs.remove(index);
            }
        }
        self.next += 1;
        let now = Instant::now();
        let state = Arc::new(Mutex::new(Tracked {
            started: now,
            advanced: now,
            run: ScanRun {
                round: self.next,
                scope_ids: scopes,
                reason: reason.into(),
                phase: "counting".into(),
                ..ScanRun::default()
            },
        }));
        self.runs.push(state.clone());
        Run(state)
    }
    pub fn snapshot(&self) -> Vec<ScanRun> {
        self.runs
            .iter()
            .map(|r| {
                let state = r.lock().unwrap();
                let mut run = state.run.clone();
                if !run.finished {
                    run.elapsed_ms = state.started.elapsed().as_millis() as u64;
                    run.idle_ms = state.advanced.elapsed().as_millis() as u64;
                }
                run
            })
            .collect()
    }
}
impl Run {
    pub fn update(&self, advanced: bool, update: impl FnOnce(&mut ScanRun)) {
        let mut state = self.0.lock().unwrap();
        update(&mut state.run);
        if advanced {
            state.advanced = Instant::now();
        }
    }
    pub fn finish(&self, phase: &str) {
        let mut state = self.0.lock().unwrap();
        state.run.phase = phase.into();
        state.run.finished = true;
        state.run.elapsed_ms = state.started.elapsed().as_millis() as u64;
        state.run.idle_ms = 0;
    }
}
impl Drop for Run {
    fn drop(&mut self) {
        if !self.0.lock().unwrap().run.finished {
            self.finish("failed");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn a_stalled_scope_does_not_accumulate_other_scopes_rescan_history() {
        let mut registry = Registry::default();
        let stalled = registry.start(vec!["slow disk".into()], "refresh");
        for _ in 0..100 {
            let other = registry.start(vec!["busy disk".into()], "watch overflow");
            assert_eq!(registry.snapshot().len(), 2);
            other.finish("complete");
        }
        assert!(!registry.snapshot()[0].finished);
        stalled.finish("cancelled");
    }

    #[test]
    fn only_real_work_resets_stall_clock_and_completed_totals_are_retained() {
        let mut registry = Registry::default();
        let first = registry.start(vec!["a".into()], "refresh");
        let second = registry.start(vec!["b".into()], "refresh");
        first.0.lock().unwrap().advanced = Instant::now() - Duration::from_secs(70);
        first.update(false, |r| r.current_path = "slow path".into());
        assert!(registry.snapshot()[0].idle_ms >= 70000);
        first.update(true, |r| {
            r.total_directories = Some(4);
            r.processed_directories = 4;
        });
        assert!(registry.snapshot()[0].idle_ms < 1000);
        first.finish("complete");
        assert_eq!(registry.snapshot().len(), 2);
        assert_eq!(registry.snapshot()[0].total_directories, Some(4));
        drop(second);
        assert_eq!(registry.snapshot()[1].phase, "failed");
        let third = registry.start(vec!["c".into()], "watch overflow");
        assert_eq!(registry.snapshot().len(), 1);
        assert_eq!(registry.snapshot()[0].round, 3);
        third.finish("cancelled");
    }
}
