//! # cine-jobs — background work
//!
//! Professional media work is long: verifying a card, generating thumbnails,
//! building proxies, rendering, delivering. None of it may make an interface
//! wait, and all of it has to survive being cancelled, failing, and the
//! application being closed halfway through.
//!
//! This crate is the machinery, and knows nothing about media. A kind of work
//! is a [`Runner`] registered under a name; a request for work is a
//! [`JobSpec`]; what the system knows about a request over its life is a
//! [`JobRecord`].
//!
//! ## What it guarantees
//!
//! * **Bounded by resource.** Each [`Resource`] class has its own worker count,
//!   so a disk-bound checksum does not starve the GPU, and four renders do not
//!   run on a two-core machine because four were asked for.
//! * **Priority, then order.** Among ready jobs of a class, the highest priority
//!   runs first; ties run in submission order.
//! * **Dependencies.** A job waits for the jobs it depends on and fails, saying
//!   which, if one of them did not succeed.
//! * **Cancellation is cooperative and final.** A queued job is cancelled at
//!   once; a running one is told through [`JobContext::is_cancelled`], and a
//!   runner that ignores it is still recorded as cancelled when it returns.
//! * **Retries are bounded.** A failure is retried until `max_attempts`, and the
//!   last error is kept.
//! * **A panicking runner fails its job**, not the application.
//! * **One job per identity.** Submitting a spec whose `key` matches a job that
//!   is still pending or already succeeded returns that job instead of doing
//!   the work twice.
//! * **Persistence.** With a journal, every state change is appended as a line
//!   of JSON. Reopening restores the history; work that was queued or running
//!   when the process stopped is queued again, and says so. Runners must
//!   therefore be safe to run again from the start — which their keys make
//!   explicit.
//!
//! ## What it does not do
//!
//! It does not checkpoint inside a job: a resumed job starts over. It does not
//! distribute work across machines. It does not decide *what* work is needed —
//! the runtime and reactors do.

use parking_lot::{Condvar, Mutex};
use serde::{Deserialize, Serialize};
use serde_json::Value as J;
use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// The class of machine resource a job consumes most.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Resource {
    /// Decoding, hashing in memory, analysis.
    Cpu,
    /// Anything that needs the GPU; usually one at a time.
    Gpu,
    /// Reading and writing large files.
    Io,
}

impl Resource {
    pub const ALL: [Resource; 3] = [Resource::Cpu, Resource::Gpu, Resource::Io];
    pub fn as_str(&self) -> &'static str {
        match self {
            Resource::Cpu => "cpu",
            Resource::Gpu => "gpu",
            Resource::Io => "io",
        }
    }
}

/// Identifies a job for its whole life, across restarts.
pub type JobId = u64;

/// A request for work.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct JobSpec {
    /// Which runner does the work.
    pub kind: String,
    /// What the work *is*: two specs with the same key are the same work.
    pub key: String,
    /// For people.
    pub label: String,
    #[serde(default)]
    pub params: J,
    #[serde(default)]
    pub priority: i32,
    pub resource: Resource,
    #[serde(default)]
    pub depends_on: Vec<JobId>,
    #[serde(default = "one")]
    pub max_attempts: u32,
}

fn one() -> u32 {
    1
}

impl JobSpec {
    pub fn new(kind: &str, key: &str, label: &str, resource: Resource) -> JobSpec {
        JobSpec {
            kind: kind.into(),
            key: key.into(),
            label: label.into(),
            params: J::Null,
            priority: 0,
            resource,
            depends_on: Vec::new(),
            max_attempts: 1,
        }
    }
    pub fn params(mut self, params: J) -> JobSpec {
        self.params = params;
        self
    }
    pub fn priority(mut self, priority: i32) -> JobSpec {
        self.priority = priority;
        self
    }
    pub fn after(mut self, deps: &[JobId]) -> JobSpec {
        self.depends_on = deps.to_vec();
        self
    }
    pub fn attempts(mut self, n: u32) -> JobSpec {
        self.max_attempts = n.max(1);
        self
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    Queued,
    Running,
    Succeeded,
    Failed,
    Cancelled,
}

impl JobState {
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            JobState::Succeeded | JobState::Failed | JobState::Cancelled
        )
    }
    pub fn as_str(&self) -> &'static str {
        match self {
            JobState::Queued => "queued",
            JobState::Running => "running",
            JobState::Succeeded => "succeeded",
            JobState::Failed => "failed",
            JobState::Cancelled => "cancelled",
        }
    }
}

/// Everything known about one job.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct JobRecord {
    pub id: JobId,
    pub spec: JobSpec,
    pub state: JobState,
    /// 0.0 ..= 1.0, when the runner reports it.
    #[serde(default)]
    pub progress: Option<f32>,
    /// What the runner last said it was doing.
    #[serde(default)]
    pub message: Option<String>,
    pub attempts: u32,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub output: Option<J>,
    pub submitted_ms: u64,
    #[serde(default)]
    pub started_ms: Option<u64>,
    #[serde(default)]
    pub finished_ms: Option<u64>,
    /// Set when the job was restored from the journal and queued again.
    #[serde(default)]
    pub resumed: bool,
}

impl JobRecord {
    /// Wall time spent running, when it has started.
    pub fn elapsed(&self) -> Option<Duration> {
        let start = self.started_ms?;
        let end = self.finished_ms.unwrap_or_else(now_ms);
        Some(Duration::from_millis(end.saturating_sub(start)))
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Handed to a runner while it works.
pub struct JobContext {
    id: JobId,
    cancel: Arc<AtomicBool>,
    inner: Arc<Inner>,
}

impl JobContext {
    pub fn id(&self) -> JobId {
        self.id
    }

    /// Whether the job has been cancelled. Long work should check this between
    /// units and return promptly; what it returns is then discarded.
    pub fn is_cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    /// Report progress. Not journalled — progress is for watching, and the
    /// journal is for what happened — and throttled by the caller's cadence.
    pub fn progress(&self, fraction: f32, message: impl Into<String>) {
        let record = {
            let mut state = self.inner.state.lock();
            let Some(job) = state.jobs.get_mut(&self.id) else {
                return;
            };
            job.record.progress = Some(fraction.clamp(0.0, 1.0));
            job.record.message = Some(message.into());
            job.record.clone()
        };
        self.inner.notify(&record);
    }
}

/// Does one kind of work.
pub trait Runner: Send + Sync {
    /// Do the work. `Ok` carries a small JSON result for the record.
    fn run(&self, ctx: &JobContext, spec: &JobSpec) -> Result<J, String>;
}

impl<F> Runner for F
where
    F: Fn(&JobContext, &JobSpec) -> Result<J, String> + Send + Sync,
{
    fn run(&self, ctx: &JobContext, spec: &JobSpec) -> Result<J, String> {
        self(ctx, spec)
    }
}

/// How many workers each resource class gets.
#[derive(Clone, Debug)]
pub struct JobConfig {
    pub workers: HashMap<Resource, usize>,
    /// Append-only record of every state change. `None` keeps nothing.
    pub journal: Option<PathBuf>,
}

impl JobConfig {
    /// One CPU worker fewer than the machine has cores (at least one), one GPU
    /// worker, two I/O workers.
    pub fn for_this_machine(journal: Option<PathBuf>) -> JobConfig {
        let cores = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1);
        JobConfig {
            workers: HashMap::from([
                (Resource::Cpu, cores.saturating_sub(1).max(1)),
                (Resource::Gpu, 1),
                (Resource::Io, 2),
            ]),
            journal,
        }
    }
}

struct Job {
    record: JobRecord,
    cancel: Arc<AtomicBool>,
}

struct State {
    jobs: HashMap<JobId, Job>,
    next_id: JobId,
    stopping: bool,
}

type Listener = Box<dyn Fn(&JobRecord) + Send + Sync>;

struct Inner {
    state: Mutex<State>,
    wake: Condvar,
    runners: Mutex<HashMap<String, Arc<dyn Runner>>>,
    listeners: Mutex<Vec<Listener>>,
    journal: Mutex<Option<File>>,
}

impl Inner {
    fn notify(&self, record: &JobRecord) {
        for l in self.listeners.lock().iter() {
            l(record);
        }
    }

    fn journal(&self, record: &JobRecord) {
        let mut journal = self.journal.lock();
        if let Some(f) = journal.as_mut() {
            // A journal that cannot be written loses history, not work: the
            // job itself is unaffected. It is reported rather than ignored.
            let line = serde_json::to_string(record).unwrap_or_default();
            if writeln!(f, "{line}").and_then(|_| f.flush()).is_err() {
                eprintln!("cine-jobs: could not append to the job journal");
            }
        }
    }

    /// Record a state change: journal it, then tell listeners.
    fn changed(&self, record: &JobRecord) {
        self.journal(record);
        self.notify(record);
    }
}

/// The job system. Cheap to clone; every clone is the same system.
#[derive(Clone)]
pub struct JobSystem {
    inner: Arc<Inner>,
    workers: Arc<Mutex<Vec<JoinHandle<()>>>>,
}

impl JobSystem {
    /// Start the workers, restoring history from the journal when there is one.
    pub fn start(config: JobConfig) -> std::io::Result<JobSystem> {
        let mut jobs = HashMap::new();
        let mut next_id = 1;
        let mut file = None;
        if let Some(path) = &config.journal {
            let restored = restore(path)?;
            for mut record in restored {
                next_id = next_id.max(record.id + 1);
                if !record.state.is_terminal() {
                    record.state = JobState::Queued;
                    record.progress = None;
                    record.started_ms = None;
                    record.resumed = true;
                    record.message = Some("queued again after the application restarted".into());
                }
                jobs.insert(
                    record.id,
                    Job {
                        record,
                        cancel: Arc::new(AtomicBool::new(false)),
                    },
                );
            }
            // Compact: one line per job, so the journal does not grow with
            // every progress-free transition forever.
            if let Some(dir) = path.parent() {
                fs::create_dir_all(dir)?;
            }
            let tmp = path.with_extension("jsonl.tmp");
            {
                let mut out = File::create(&tmp)?;
                let mut ids: Vec<_> = jobs.keys().copied().collect();
                ids.sort_unstable();
                for id in ids {
                    writeln!(out, "{}", serde_json::to_string(&jobs[&id].record)?)?;
                }
                out.flush()?;
            }
            fs::rename(&tmp, path)?;
            file = Some(OpenOptions::new().append(true).open(path)?);
        }
        let inner = Arc::new(Inner {
            state: Mutex::new(State {
                jobs,
                next_id,
                stopping: false,
            }),
            wake: Condvar::new(),
            runners: Mutex::new(HashMap::new()),
            listeners: Mutex::new(Vec::new()),
            journal: Mutex::new(file),
        });
        let mut handles = Vec::new();
        for resource in Resource::ALL {
            let n = config.workers.get(&resource).copied().unwrap_or(1).max(1);
            for i in 0..n {
                let inner = inner.clone();
                handles.push(
                    std::thread::Builder::new()
                        .name(format!("cine-job-{}-{i}", resource.as_str()))
                        .spawn(move || worker(inner, resource))?,
                );
            }
        }
        Ok(JobSystem {
            inner,
            workers: Arc::new(Mutex::new(handles)),
        })
    }

    /// Make a kind of work available. Jobs of an unregistered kind wait; a job
    /// restored from the journal can therefore be picked up once the
    /// application has registered its runners.
    pub fn register(&self, kind: &str, runner: impl Runner + 'static) {
        self.inner
            .runners
            .lock()
            .insert(kind.to_string(), Arc::new(runner));
        self.inner.wake.notify_all();
    }

    /// Hear about every change to every job, progress included.
    pub fn subscribe(&self, f: impl Fn(&JobRecord) + Send + Sync + 'static) {
        self.inner.listeners.lock().push(Box::new(f));
    }

    /// Ask for work. Returns the job doing it — an existing one when the same
    /// work is already pending or done.
    pub fn submit(&self, spec: JobSpec) -> JobId {
        let record = {
            let mut state = self.inner.state.lock();
            if let Some(existing) = state.jobs.values().find(|j| {
                j.record.spec.key == spec.key
                    && matches!(
                        j.record.state,
                        JobState::Queued | JobState::Running | JobState::Succeeded
                    )
            }) {
                return existing.record.id;
            }
            let id = state.next_id;
            state.next_id += 1;
            let record = JobRecord {
                id,
                spec,
                state: JobState::Queued,
                progress: None,
                message: None,
                attempts: 0,
                error: None,
                output: None,
                submitted_ms: now_ms(),
                started_ms: None,
                finished_ms: None,
                resumed: false,
            };
            state.jobs.insert(
                id,
                Job {
                    record: record.clone(),
                    cancel: Arc::new(AtomicBool::new(false)),
                },
            );
            record
        };
        self.inner.changed(&record);
        self.inner.wake.notify_all();
        record.id
    }

    /// Cancel a job that has not finished. Returns whether it was cancellable.
    pub fn cancel(&self, id: JobId) -> bool {
        let record = {
            let mut state = self.inner.state.lock();
            let Some(job) = state.jobs.get_mut(&id) else {
                return false;
            };
            match job.record.state {
                JobState::Queued => {
                    job.cancel.store(true, Ordering::Relaxed);
                    job.record.state = JobState::Cancelled;
                    job.record.finished_ms = Some(now_ms());
                    job.record.clone()
                }
                JobState::Running => {
                    // The runner finds out and returns; the worker records it.
                    job.cancel.store(true, Ordering::Relaxed);
                    return true;
                }
                _ => return false,
            }
        };
        self.inner.changed(&record);
        self.inner.wake.notify_all();
        true
    }

    /// Run a failed or cancelled job again, with a fresh attempt budget.
    pub fn retry(&self, id: JobId) -> bool {
        let record = {
            let mut state = self.inner.state.lock();
            let Some(job) = state.jobs.get_mut(&id) else {
                return false;
            };
            if !matches!(job.record.state, JobState::Failed | JobState::Cancelled) {
                return false;
            }
            job.cancel = Arc::new(AtomicBool::new(false));
            job.record.state = JobState::Queued;
            job.record.attempts = 0;
            job.record.error = None;
            job.record.progress = None;
            job.record.started_ms = None;
            job.record.finished_ms = None;
            job.record.clone()
        };
        self.inner.changed(&record);
        self.inner.wake.notify_all();
        true
    }

    pub fn get(&self, id: JobId) -> Option<JobRecord> {
        self.inner
            .state
            .lock()
            .jobs
            .get(&id)
            .map(|j| j.record.clone())
    }

    /// Every job, newest first.
    pub fn list(&self) -> Vec<JobRecord> {
        let mut out: Vec<JobRecord> = self
            .inner
            .state
            .lock()
            .jobs
            .values()
            .map(|j| j.record.clone())
            .collect();
        out.sort_by_key(|r| std::cmp::Reverse(r.id));
        out
    }

    /// Block until `id` is terminal or `timeout` passes. For tests and tools.
    pub fn wait(&self, id: JobId, timeout: Duration) -> Option<JobRecord> {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            match self.get(id) {
                Some(r) if r.state.is_terminal() => return Some(r),
                None => return None,
                _ => {}
            }
            if std::time::Instant::now() >= deadline {
                return self.get(id);
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// Stop taking work, tell running jobs to stop, and join the workers.
    /// Jobs still queued stay queued in the journal and resume next start.
    pub fn shutdown(&self) {
        {
            let mut state = self.inner.state.lock();
            state.stopping = true;
            for job in state.jobs.values() {
                if job.record.state == JobState::Running {
                    job.cancel.store(true, Ordering::Relaxed);
                }
            }
        }
        self.inner.wake.notify_all();
        for h in self.workers.lock().drain(..) {
            let _ = h.join();
        }
    }
}

fn restore(path: &Path) -> std::io::Result<Vec<JobRecord>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let mut last: HashMap<JobId, JobRecord> = HashMap::new();
    for line in BufReader::new(File::open(path)?).lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        // A torn final line from a killed process is skipped; the job's
        // previous line still describes it.
        if let Ok(r) = serde_json::from_str::<JobRecord>(&line) {
            last.insert(r.id, r);
        }
    }
    let mut out: Vec<JobRecord> = last.into_values().collect();
    out.sort_by_key(|r| r.id);
    Ok(out)
}

enum Pick {
    Run(JobId, JobSpec, Arc<AtomicBool>, Arc<dyn Runner>),
    /// Already marked failed under the lock, so no other worker can pick it.
    Failed(JobRecord),
}

/// The next thing a worker of `resource` should do, if anything.
fn pick(
    state: &mut State,
    runners: &HashMap<String, Arc<dyn Runner>>,
    resource: Resource,
) -> Option<Pick> {
    let mut best: Option<(i32, JobId)> = None;
    let mut failed: Option<(JobId, String)> = None;
    for job in state.jobs.values() {
        let r = &job.record;
        if r.state != JobState::Queued || r.spec.resource != resource {
            continue;
        }
        // A dependency that did not succeed fails its dependants.
        let mut blocked = false;
        for dep in &r.spec.depends_on {
            match state.jobs.get(dep).map(|d| d.record.state) {
                Some(JobState::Succeeded) => {}
                Some(JobState::Failed) | Some(JobState::Cancelled) | None => {
                    failed = Some((r.id, format!("dependency {dep} did not succeed")));
                    break;
                }
                _ => blocked = true,
            }
        }
        if failed.is_some() {
            break;
        }
        if blocked || !runners.contains_key(&r.spec.kind) {
            continue;
        }
        let candidate = (r.spec.priority, r.id);
        best = match best {
            Some((p, id)) if p > candidate.0 || (p == candidate.0 && id < candidate.1) => {
                Some((p, id))
            }
            _ => Some(candidate),
        };
    }
    if let Some((id, why)) = failed {
        let job = state.jobs.get_mut(&id)?;
        job.record.state = JobState::Failed;
        job.record.error = Some(why);
        job.record.finished_ms = Some(now_ms());
        return Some(Pick::Failed(job.record.clone()));
    }
    let (_, id) = best?;
    let job = state.jobs.get_mut(&id)?;
    job.record.state = JobState::Running;
    job.record.attempts += 1;
    job.record.started_ms = Some(now_ms());
    job.record.progress = None;
    let runner = runners.get(&job.record.spec.kind)?.clone();
    Some(Pick::Run(
        id,
        job.record.spec.clone(),
        job.cancel.clone(),
        runner,
    ))
}

fn worker(inner: Arc<Inner>, resource: Resource) {
    loop {
        let picked = {
            let mut state = inner.state.lock();
            loop {
                if state.stopping {
                    return;
                }
                let runners = inner.runners.lock().clone();
                if let Some(p) = pick(&mut state, &runners, resource) {
                    break p;
                }
                inner.wake.wait_for(&mut state, Duration::from_millis(500));
            }
        };
        match picked {
            Pick::Failed(record) => {
                inner.changed(&record);
                inner.wake.notify_all();
            }
            Pick::Run(id, spec, cancel, runner) => {
                if let Some(r) = inner.state.lock().jobs.get(&id).map(|j| j.record.clone()) {
                    inner.changed(&r);
                }
                let ctx = JobContext {
                    id,
                    cancel: cancel.clone(),
                    inner: inner.clone(),
                };
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    runner.run(&ctx, &spec)
                }))
                .unwrap_or_else(|panic| {
                    let why = panic
                        .downcast_ref::<String>()
                        .cloned()
                        .or_else(|| panic.downcast_ref::<&str>().map(|s| s.to_string()))
                        .unwrap_or_else(|| "the runner panicked".into());
                    Err(format!("panicked: {why}"))
                });
                let record = if cancel.load(Ordering::Relaxed) {
                    finish(&inner, id, JobState::Cancelled, None, None)
                } else {
                    match result {
                        Ok(out) => finish(&inner, id, JobState::Succeeded, Some(out), None),
                        Err(e) => {
                            let retry =
                                inner.state.lock().jobs.get(&id).is_some_and(|j| {
                                    j.record.attempts < j.record.spec.max_attempts
                                });
                            if retry {
                                requeue(&inner, id, e)
                            } else {
                                finish(&inner, id, JobState::Failed, None, Some(e))
                            }
                        }
                    }
                };
                if let Some(r) = record {
                    inner.changed(&r);
                }
                inner.wake.notify_all();
            }
        }
    }
}

fn finish(
    inner: &Inner,
    id: JobId,
    to: JobState,
    output: Option<J>,
    error: Option<String>,
) -> Option<JobRecord> {
    let mut state = inner.state.lock();
    let job = state.jobs.get_mut(&id)?;
    job.record.state = to;
    job.record.finished_ms = Some(now_ms());
    if to == JobState::Succeeded {
        job.record.progress = Some(1.0);
    }
    job.record.output = output;
    job.record.error = error;
    Some(job.record.clone())
}

fn requeue(inner: &Inner, id: JobId, error: String) -> Option<JobRecord> {
    let mut state = inner.state.lock();
    let job = state.jobs.get_mut(&id)?;
    job.record.state = JobState::Queued;
    job.record.message = Some(format!("attempt {} failed: {error}", job.record.attempts));
    job.record.error = Some(error);
    Some(job.record.clone())
}
