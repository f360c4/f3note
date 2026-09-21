//! The autosave engine.
//!
//! What this has to guarantee is simple to state and easy to get wrong: at any
//! moment the machine loses power, whatever the user last typed is recoverable.
//! Everything below follows from that.
//!
//! **Nothing is written on a timer.** Writing every N seconds regardless of
//! activity means an editor left open overnight does thousands of pointless
//! writes. Work is triggered by the buffer actually changing, then debounced —
//! the mirror is written once the user pauses, with a ceiling so that someone
//! typing continuously still gets saved regularly.
//!
//! **The expensive half runs off the main thread.** Reading a buffer's text has
//! to happen on the main thread, because GTK is not thread-safe. Hashing it,
//! compressing it and writing it do not, and on a large document they are
//! easily tens of milliseconds — long enough to be felt as a stutter while
//! typing. Only the copy crosses the thread boundary.
//!
//! **Failures are surfaced, not swallowed.** A full disk means autosave has
//! stopped protecting the user's work, and they need to hear about it. Linux
//! reports a writeback error once and then forgets it, so retrying quietly is
//! the one thing that must not happen.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};

use crate::session::store::DocStore;

/// What the worker thread is asked to do with one document's contents.
pub struct Job {
    pub key: String,
    pub contents: Vec<u8>,
    /// False for documents too large to keep history for; the mirror is still
    /// written, because recovery matters regardless of size.
    pub keep_history: bool,
    pub history_limit: usize,
}

/// What came back.
pub enum Outcome {
    Written {
        key: String,
    },
    /// Nothing needed doing: the contents were already mirrored.
    Unchanged {
        key: String,
    },
    Failed {
        key: String,
        error: String,
    },
}

/// Runs the disk work for the autosave engine on a thread of its own.
pub struct Worker {
    jobs: Sender<Job>,
    outcomes: Receiver<Outcome>,
}

impl Worker {
    pub fn spawn(root: PathBuf) -> Worker {
        let (job_tx, job_rx) = mpsc::channel::<Job>();
        let (outcome_tx, outcome_rx) = mpsc::channel::<Outcome>();

        std::thread::Builder::new()
            .name("f3note-autosave".to_owned())
            .spawn(move || {
                // The loop ends when the sender is dropped, which happens when
                // the editor shuts down.
                for job in job_rx {
                    let store = DocStore::new(&root, &job.key);
                    let outcome = match store.write_mirror(&job.contents) {
                        Ok(None) => Outcome::Unchanged { key: job.key },
                        Ok(Some(_)) => {
                            if job.keep_history {
                                // History is best-effort by design: the mirror
                                // is what recovery reads, and a failure to keep
                                // an extra version must never look like a
                                // failure to protect the current text.
                                if let Err(e) = store.push_history(&job.contents, job.history_limit)
                                {
                                    eprintln!("f3note: history for {}: {e}", job.key);
                                }
                            }
                            Outcome::Written { key: job.key }
                        }
                        Err(e) => Outcome::Failed {
                            key: job.key,
                            error: e.to_string(),
                        },
                    };
                    if outcome_tx.send(outcome).is_err() {
                        break;
                    }
                }
            })
            .expect("spawning the autosave thread");

        Worker {
            jobs: job_tx,
            outcomes: outcome_rx,
        }
    }

    pub fn submit(&self, job: Job) {
        // A closed channel means the worker is gone, which only happens during
        // shutdown; there is nothing useful to do about it here.
        let _ = self.jobs.send(job);
    }

    /// Collect whatever the worker has finished since the last call. Never
    /// blocks.
    pub fn drain(&self) -> Vec<Outcome> {
        self.outcomes.try_iter().collect()
    }
}

/// Decides *when* a mirror should be written.
///
/// Kept separate from the GTK plumbing so the policy — the part that is easy to
/// get subtly wrong — can be tested without a display.
pub struct Schedule {
    idle: std::time::Duration,
    ceiling: std::time::Duration,
    dirty: HashSet<u64>,
    /// When each document first became dirty without being written since.
    first_dirty: HashMap<u64, std::time::Instant>,
    last_change: Option<std::time::Instant>,
}

impl Schedule {
    pub fn new(idle_seconds: u64, ceiling_seconds: u64) -> Schedule {
        Schedule {
            idle: std::time::Duration::from_secs(idle_seconds),
            ceiling: std::time::Duration::from_secs(ceiling_seconds.max(idle_seconds)),
            dirty: HashSet::new(),
            first_dirty: HashMap::new(),
            last_change: None,
        }
    }

    pub fn mark_dirty(&mut self, id: u64, now: std::time::Instant) {
        self.dirty.insert(id);
        self.first_dirty.entry(id).or_insert(now);
        self.last_change = Some(now);
    }

    pub fn is_dirty(&self, id: u64) -> bool {
        self.dirty.contains(&id)
    }

    pub fn has_work(&self) -> bool {
        !self.dirty.is_empty()
    }

    /// Which documents should be written right now.
    ///
    /// Either the user has paused for long enough, or a document has been
    /// waiting since before the ceiling and must not wait any longer. The
    /// second case is what protects someone who types without stopping.
    pub fn due(&self, now: std::time::Instant) -> Vec<u64> {
        if self.dirty.is_empty() {
            return Vec::new();
        }
        let paused = self
            .last_change
            .map(|t| now.duration_since(t) >= self.idle)
            .unwrap_or(false);
        if paused {
            let mut all: Vec<u64> = self.dirty.iter().copied().collect();
            all.sort_unstable();
            return all;
        }
        let mut overdue: Vec<u64> = self
            .dirty
            .iter()
            .filter(|id| {
                self.first_dirty
                    .get(id)
                    .map(|since| now.duration_since(*since) >= self.ceiling)
                    .unwrap_or(false)
            })
            .copied()
            .collect();
        overdue.sort_unstable();
        overdue
    }

    /// Note that a document has been handed to the worker.
    pub fn clear(&mut self, id: u64) {
        self.dirty.remove(&id);
        self.first_dirty.remove(&id);
    }

    pub fn forget(&mut self, id: u64) {
        self.clear(id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn nothing_is_due_when_nothing_changed() {
        let s = Schedule::new(2, 7);
        assert!(!s.has_work());
        assert!(s.due(Instant::now()).is_empty());
    }

    #[test]
    fn a_pause_makes_every_dirty_document_due() {
        let mut s = Schedule::new(2, 7);
        let t0 = Instant::now();
        s.mark_dirty(1, t0);
        s.mark_dirty(2, t0);
        // Still typing: too early.
        assert!(s.due(t0 + Duration::from_millis(500)).is_empty());
        // Paused for longer than the idle delay.
        assert_eq!(s.due(t0 + Duration::from_secs(3)), vec![1, 2]);
    }

    #[test]
    fn continuous_typing_still_gets_written_at_the_ceiling() {
        let mut s = Schedule::new(2, 7);
        let t0 = Instant::now();
        s.mark_dirty(1, t0);
        // Someone typing without ever pausing: the idle rule never fires, so
        // the ceiling is the only thing protecting them.
        for second in 1..7 {
            let now = t0 + Duration::from_secs(second);
            s.mark_dirty(1, now);
            assert!(s.due(now).is_empty(), "too early at {second}s");
        }
        let now = t0 + Duration::from_secs(7);
        s.mark_dirty(1, now);
        assert_eq!(s.due(now), vec![1], "the ceiling must force a write");
    }

    #[test]
    fn writing_clears_the_document_until_it_changes_again() {
        let mut s = Schedule::new(2, 7);
        let t0 = Instant::now();
        s.mark_dirty(1, t0);
        s.clear(1);
        assert!(!s.has_work());
        assert!(s.due(t0 + Duration::from_secs(10)).is_empty());

        s.mark_dirty(1, t0 + Duration::from_secs(10));
        assert_eq!(s.due(t0 + Duration::from_secs(13)), vec![1]);
    }

    #[test]
    fn the_ceiling_can_never_be_shorter_than_the_idle_delay() {
        // A config with the two the wrong way round must not produce a
        // schedule that writes before the user has stopped typing.
        let s = Schedule::new(30, 5);
        assert!(s.ceiling >= s.idle);
    }

    #[test]
    fn a_forgotten_document_stops_being_scheduled() {
        let mut s = Schedule::new(2, 7);
        let t0 = Instant::now();
        s.mark_dirty(7, t0);
        s.forget(7);
        assert!(!s.is_dirty(7));
        assert!(s.due(t0 + Duration::from_secs(5)).is_empty());
    }

    #[test]
    fn the_worker_writes_a_mirror_and_reports_back() {
        let root = std::env::temp_dir().join(format!("f3note_worker_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let worker = Worker::spawn(root.clone());

        worker.submit(Job {
            key: "k".to_owned(),
            contents: b"hello from the worker".to_vec(),
            keep_history: true,
            history_limit: 5,
        });

        // The worker is on another thread, so wait for it rather than assuming.
        let mut outcomes = Vec::new();
        for _ in 0..200 {
            outcomes.extend(worker.drain());
            if !outcomes.is_empty() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(matches!(outcomes.first(), Some(Outcome::Written { .. })));
        assert_eq!(
            DocStore::new(&root, "k").read_mirror().unwrap(),
            b"hello from the worker"
        );
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn resubmitting_identical_contents_reports_unchanged() {
        let root = std::env::temp_dir().join(format!("f3note_worker2_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let worker = Worker::spawn(root.clone());
        let job = || Job {
            key: "k".to_owned(),
            contents: b"same".to_vec(),
            keep_history: true,
            history_limit: 5,
        };

        worker.submit(job());
        worker.submit(job());

        let mut outcomes = Vec::new();
        for _ in 0..200 {
            outcomes.extend(worker.drain());
            if outcomes.len() >= 2 {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(matches!(outcomes[0], Outcome::Written { .. }));
        assert!(
            matches!(outcomes[1], Outcome::Unchanged { .. }),
            "an unchanged buffer must not be rewritten"
        );
        std::fs::remove_dir_all(root).ok();
    }
}
