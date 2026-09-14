//! Admit full scans by filesystem volume and coalesce pending overlapping roots.
use crate::{platform::*, scan, Control, Engine};
use anyhow::Result;
use std::{
    collections::VecDeque,
    path::PathBuf,
    sync::{atomic::Ordering, Arc, Condvar, Mutex},
    time::Duration,
};

struct Request {
    ticket: u64,
    root: PathBuf,
    volumes: Vec<String>,
}

impl Request {
    fn conflicts(&self, other: &Self) -> bool {
        self.root.starts_with(&other.root)
            || other.root.starts_with(&self.root)
            || self.volumes.iter().any(|v| other.volumes.contains(v))
    }
}
#[derive(Default)]
struct State {
    next: u64,
    waiting: VecDeque<Request>,
    active: Vec<Request>,
}
#[derive(Default)]
pub(crate) struct Scheduler {
    state: Mutex<State>,
    changed: Condvar,
}
struct Permit<'a> {
    scheduler: &'a Scheduler,
    ticket: u64,
}
impl Scheduler {
    fn enter(
        &self,
        root: PathBuf,
        volumes: Vec<String>,
        needed: impl Fn() -> bool,
    ) -> Option<Permit<'_>> {
        let mut state = self.state.lock().unwrap();
        let ticket = state.next;
        state.next += 1;
        state.waiting.push_back(Request {
            ticket,
            root,
            volumes,
        });
        loop {
            let at = state
                .waiting
                .iter()
                .position(|r| r.ticket == ticket)
                .unwrap();
            if !needed() {
                state.waiting.remove(at);
                self.changed.notify_all();
                return None;
            }
            let request = &state.waiting[at];
            if state.active.len() < 4
                && !state.active.iter().any(|a| a.conflicts(request))
                && !state.waiting.iter().take(at).any(|a| a.conflicts(request))
            {
                let request = state.waiting.remove(at).unwrap();
                state.active.push(request);
                return Some(Permit {
                    scheduler: self,
                    ticket,
                });
            }
            // Also observe cancellation, removal, or work claimed by a parent scan.
            state = self
                .changed
                .wait_timeout(state, Duration::from_millis(100))
                .unwrap()
                .0;
        }
    }
}
impl Drop for Permit<'_> {
    fn drop(&mut self) {
        self.scheduler
            .state
            .lock()
            .unwrap()
            .active
            .retain(|r| r.ticket != self.ticket);
        self.scheduler.changed.notify_all();
    }
}

fn needed(control: &Control) -> bool {
    if control.cancel.load(Ordering::SeqCst) || control.removed.load(Ordering::SeqCst) {
        return false;
    }
    let pending = control.pending.lock().unwrap();
    pending.full || !pending.paths.is_empty()
}
impl Engine {
    pub(crate) fn scan_pending(&self, id: &str, control: &Arc<Control>) -> Result<()> {
        if !needed(control) {
            return Ok(());
        }
        let roots: Vec<(String, PathBuf, String)> = {
            let c = self.lock()?;
            let mut statement = c.prepare("SELECT id,root,identity FROM scopes")?;
            let rows = statement.query_map([], |r| {
                Ok((
                    r.get(0)?,
                    PathBuf::from(decode(&r.get::<_, Vec<u8>>(1)?)),
                    r.get(2)?,
                ))
            })?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        let Some((_, own_root, own_identity)) = roots.iter().find(|r| r.0 == id) else {
            return Ok(());
        };
        // Never hold the DB lock while acquiring controls (cancel uses the reverse order).
        let controls = self.inner.controls.lock().unwrap().clone();
        let full = control.pending.lock().unwrap().full;
        let eligible: Vec<_> = roots
            .iter()
            .filter(|(scope, _, _)| {
                controls
                    .get(scope)
                    .is_some_and(|c| needed(c) && c.pending.lock().unwrap().full)
            })
            .collect();
        let leader = if full {
            eligible
                .iter()
                .filter(|(_, root, _)| own_root.starts_with(root))
                .min_by_key(|(_, root, _)| root.components().count())
                .map(|r| r.1.clone())
                .unwrap_or_else(|| own_root.clone())
        } else {
            own_root.clone()
        };
        let mut group: Vec<_> = if full {
            eligible
                .into_iter()
                // A refresh batch plans all pending roots before indexing any files.
                // The shared pool still scans directories concurrently; overlapping
                // roots share one directory plan and one set of observations.
                .collect()
        } else {
            Vec::new()
        };
        if group.is_empty() {
            group.push(roots.iter().find(|r| r.0 == id).unwrap());
        }
        // Existing native identities start with st_dev (Unix) or volume serial
        // (Windows). Separate partitions on one physical disk remain a limitation.
        let mut volumes: Vec<_> = group
            .iter()
            .map(|(_, _, identity)| identity.split(':').next().unwrap_or("unknown").to_owned())
            .collect();
        volumes.push(
            own_identity
                .split(':')
                .next()
                .unwrap_or("unknown")
                .to_owned(),
        );
        volumes.sort();
        volumes.dedup();
        let Some(_permit) = self
            .inner
            .scan_scheduler
            .enter(leader, volumes, || needed(control))
        else {
            return Ok(());
        };

        let work = {
            let mut pending = control.pending.lock().unwrap();
            if control.cancel.load(Ordering::SeqCst) || control.removed.load(Ordering::SeqCst) {
                return Ok(());
            }
            control.dirty.store(false, Ordering::SeqCst);
            std::mem::take(&mut *pending)
        };
        if work.full {
            let mut targets = vec![(id.to_owned(), control.clone())];
            for (scope, _, _) in group {
                if scope == id {
                    continue;
                }
                let Some(other) = controls.get(scope) else {
                    continue;
                };
                let mut pending = other.pending.lock().unwrap();
                if pending.full
                    && !other.cancel.load(Ordering::SeqCst)
                    && !other.removed.load(Ordering::SeqCst)
                {
                    other.dirty.store(false, Ordering::SeqCst);
                    std::mem::take(&mut *pending);
                    targets.push((scope.clone(), other.clone()));
                }
            }
            if let Err(error) =
                scan::run_group_with_reason(self, &targets, work.reason.unwrap_or("完整核对"))
            {
                for (scope, c) in targets {
                    if !c.removed.load(Ordering::SeqCst) && !c.cancel.load(Ordering::SeqCst) {
                        if let Ok(db) = self.lock() {
                            let _ = crate::db::set_state(
                                &db,
                                &scope,
                                "available",
                                "partial",
                                Some(&error.to_string()),
                            );
                        }
                    }
                }
            }
            Ok(())
        } else if work.paths.is_empty()
            || scan::update_files(
                self,
                id,
                control,
                &work.paths.into_iter().collect::<Vec<_>>(),
            )?
        {
            Ok(())
        } else {
            scan::run(self, id, control)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        sync::{atomic::AtomicBool, mpsc},
        thread,
        time::Instant,
    };

    fn wait_for_queue(scheduler: &Scheduler, count: usize) {
        let start = Instant::now();
        while scheduler.state.lock().unwrap().waiting.len() != count {
            assert!(start.elapsed() < Duration::from_secs(3));
            thread::yield_now();
        }
    }

    #[test]
    fn same_volume_and_overlapping_paths_serialize_but_independent_volumes_proceed() {
        for (second_root, second_volume) in [("right", "disk-a"), ("left/child", "disk-b")] {
            let scheduler = Scheduler::default();
            let first = scheduler
                .enter("left".into(), vec!["disk-a".into()], || true)
                .unwrap();
            thread::scope(|threads| {
                let (sender, receiver) = mpsc::channel();
                let scheduler_ref = &scheduler;
                let worker = threads.spawn(move || {
                    let _permit = scheduler_ref
                        .enter(second_root.into(), vec![second_volume.into()], || true)
                        .unwrap();
                    sender.send(()).unwrap();
                });
                wait_for_queue(&scheduler, 1);
                assert!(receiver.try_recv().is_err());
                let independent = scheduler
                    .enter("independent".into(), vec!["disk-c".into()], || true)
                    .unwrap();
                assert_eq!(scheduler.state.lock().unwrap().active.len(), 2);
                drop(first);
                receiver.recv_timeout(Duration::from_secs(3)).unwrap();
                worker.join().unwrap();
                drop(independent);
            });
            assert!(scheduler.state.lock().unwrap().active.is_empty());
        }
    }

    #[test]
    fn cancelled_waiter_leaves_queue_without_waiting_for_active_scan() {
        let scheduler = Scheduler::default();
        let _first = scheduler
            .enter("left".into(), vec!["disk".into()], || true)
            .unwrap();
        let needed = AtomicBool::new(true);
        thread::scope(|threads| {
            let (sender, receiver) = mpsc::channel();
            let scheduler_ref = &scheduler;
            let needed_ref = &needed;
            let worker = threads.spawn(move || {
                let permit = scheduler_ref.enter("right".into(), vec!["disk".into()], || {
                    needed_ref.load(Ordering::SeqCst)
                });
                sender.send(permit.is_none()).unwrap();
            });
            wait_for_queue(&scheduler, 1);
            needed.store(false, Ordering::SeqCst);
            assert!(receiver.recv_timeout(Duration::from_secs(3)).unwrap());
            worker.join().unwrap();
        });
        assert!(scheduler.state.lock().unwrap().waiting.is_empty());
    }
}
