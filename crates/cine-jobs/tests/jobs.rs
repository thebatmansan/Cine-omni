//! The job system's guarantees, each observed rather than assumed.

use cine_jobs::*;
use serde_json::json;
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const T: Duration = Duration::from_secs(10);

fn system(cpu: usize) -> JobSystem {
    JobSystem::start(JobConfig {
        workers: HashMap::from([(Resource::Cpu, cpu), (Resource::Gpu, 1), (Resource::Io, 1)]),
        journal: None,
    })
    .unwrap()
}

#[test]
fn work_runs_and_reports_its_result() {
    let jobs = system(1);
    jobs.register("double", |_: &JobContext, spec: &JobSpec| {
        Ok(json!(spec.params["n"].as_i64().unwrap() * 2))
    });
    let id = jobs.submit(
        JobSpec::new("double", "d:21", "double 21", Resource::Cpu).params(json!({"n": 21})),
    );
    let r = jobs.wait(id, T).unwrap();
    assert_eq!(r.state, JobState::Succeeded);
    assert_eq!(r.output, Some(json!(42)));
    assert_eq!(r.progress, Some(1.0));
    assert_eq!(r.attempts, 1);
}

#[test]
fn higher_priority_runs_first_and_ties_run_in_order() {
    let jobs = system(1);
    let order = Arc::new(Mutex::new(Vec::new()));
    // Hold the only worker while the queue fills.
    let gate = Arc::new(Mutex::new(()));
    let held = gate.lock().unwrap();
    let g = gate.clone();
    jobs.register("hold", move |_: &JobContext, _: &JobSpec| {
        let _g = g.lock().unwrap();
        Ok(json!(null))
    });
    let o = order.clone();
    jobs.register("log", move |_: &JobContext, spec: &JobSpec| {
        o.lock().unwrap().push(spec.label.clone());
        Ok(json!(null))
    });
    let hold = jobs.submit(JobSpec::new("hold", "hold", "hold", Resource::Cpu));
    std::thread::sleep(Duration::from_millis(50));
    let a = jobs.submit(JobSpec::new("log", "a", "low-1", Resource::Cpu));
    let b = jobs.submit(JobSpec::new("log", "b", "high", Resource::Cpu).priority(10));
    let c = jobs.submit(JobSpec::new("log", "c", "low-2", Resource::Cpu));
    drop(held);
    for id in [hold, a, b, c] {
        jobs.wait(id, T);
    }
    assert_eq!(*order.lock().unwrap(), vec!["high", "low-1", "low-2"]);
}

#[test]
fn a_dependant_waits_and_fails_when_its_dependency_fails() {
    let jobs = system(2);
    jobs.register("ok", |_: &JobContext, _: &JobSpec| Ok(json!("done")));
    jobs.register("bad", |_: &JobContext, _: &JobSpec| {
        Err("disk full".to_string())
    });
    let good = jobs.submit(JobSpec::new("ok", "good", "good", Resource::Io));
    let after_good =
        jobs.submit(JobSpec::new("ok", "after-good", "after good", Resource::Cpu).after(&[good]));
    let bad = jobs.submit(JobSpec::new("bad", "bad", "bad", Resource::Io));
    let after_bad =
        jobs.submit(JobSpec::new("ok", "after-bad", "after bad", Resource::Cpu).after(&[bad]));
    assert_eq!(jobs.wait(after_good, T).unwrap().state, JobState::Succeeded);
    assert!(jobs.get(good).unwrap().finished_ms <= jobs.get(after_good).unwrap().started_ms);
    let r = jobs.wait(after_bad, T).unwrap();
    assert_eq!(r.state, JobState::Failed);
    assert!(r.error.unwrap().contains(&format!("dependency {bad}")));
    assert_eq!(r.attempts, 0, "a job that could never run did not run");
}

#[test]
fn cancelling_a_running_job_stops_it_and_records_it() {
    let jobs = system(1);
    let ticks = Arc::new(AtomicUsize::new(0));
    let t = ticks.clone();
    jobs.register("long", move |ctx: &JobContext, _: &JobSpec| {
        for i in 0..1_000 {
            if ctx.is_cancelled() {
                return Ok(json!("stopped early"));
            }
            t.fetch_add(1, Ordering::Relaxed);
            ctx.progress(i as f32 / 1_000.0, format!("step {i}"));
            std::thread::sleep(Duration::from_millis(1));
        }
        Ok(json!("finished"))
    });
    let id = jobs.submit(JobSpec::new("long", "long", "long", Resource::Cpu));
    while ticks.load(Ordering::Relaxed) < 5 {
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(jobs.cancel(id));
    let r = jobs.wait(id, T).unwrap();
    assert_eq!(r.state, JobState::Cancelled);
    assert!(
        r.output.is_none(),
        "what a cancelled job returned is discarded"
    );
    assert!(ticks.load(Ordering::Relaxed) < 1_000);
    assert!(!jobs.cancel(id), "a finished job cannot be cancelled");
    assert!(jobs.retry(id));
    assert_eq!(jobs.wait(id, T).unwrap().state, JobState::Succeeded);
}

#[test]
fn failures_are_retried_to_the_limit_and_panics_are_contained() {
    let jobs = system(1);
    let calls = Arc::new(AtomicUsize::new(0));
    let c = calls.clone();
    jobs.register("flaky", move |_: &JobContext, _: &JobSpec| {
        if c.fetch_add(1, Ordering::Relaxed) < 2 {
            Err("transient".into())
        } else {
            Ok(json!("third time"))
        }
    });
    jobs.register(
        "boom",
        |_: &JobContext, _: &JobSpec| -> Result<serde_json::Value, String> {
            panic!("decoder exploded")
        },
    );
    let flaky = jobs.submit(JobSpec::new("flaky", "flaky", "flaky", Resource::Cpu).attempts(3));
    let r = jobs.wait(flaky, T).unwrap();
    assert_eq!((r.state, r.attempts), (JobState::Succeeded, 3));

    let boom = jobs.submit(JobSpec::new("boom", "boom", "boom", Resource::Cpu).attempts(2));
    let r = jobs.wait(boom, T).unwrap();
    assert_eq!(r.state, JobState::Failed);
    assert_eq!(r.attempts, 2);
    assert!(r.error.unwrap().contains("decoder exploded"));
    // The system still works after a panic.
    let again = jobs.submit(JobSpec::new(
        "flaky",
        "flaky-2",
        "flaky again",
        Resource::Cpu,
    ));
    assert_eq!(jobs.wait(again, T).unwrap().state, JobState::Succeeded);
}

#[test]
fn the_same_work_is_not_done_twice() {
    let jobs = system(1);
    let calls = Arc::new(AtomicUsize::new(0));
    let c = calls.clone();
    jobs.register("hash", move |_: &JobContext, _: &JobSpec| {
        c.fetch_add(1, Ordering::Relaxed);
        Ok(json!("abc"))
    });
    let a = jobs.submit(JobSpec::new("hash", "hash:file1", "hash", Resource::Io));
    let b = jobs.submit(JobSpec::new(
        "hash",
        "hash:file1",
        "hash again",
        Resource::Io,
    ));
    assert_eq!(a, b);
    jobs.wait(a, T);
    let c2 = jobs.submit(JobSpec::new(
        "hash",
        "hash:file1",
        "hash a third time",
        Resource::Io,
    ));
    assert_eq!(c2, a, "succeeded work is not repeated");
    assert_eq!(calls.load(Ordering::Relaxed), 1);
}

#[test]
fn history_survives_a_restart_and_interrupted_work_resumes() {
    let dir = std::env::temp_dir().join(format!("cine-jobs-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let journal = dir.join("jobs/journal.jsonl");
    let config = || JobConfig {
        workers: HashMap::from([(Resource::Cpu, 1), (Resource::Gpu, 1), (Resource::Io, 1)]),
        journal: Some(journal.clone()),
    };
    let (done, waiting) = {
        let jobs = JobSystem::start(config()).unwrap();
        jobs.register("ok", |_: &JobContext, _: &JobSpec| Ok(json!("yes")));
        let done = jobs.submit(JobSpec::new("ok", "done", "done", Resource::Cpu));
        jobs.wait(done, T);
        // No runner for this kind in this session: it stays queued.
        let waiting = jobs.submit(JobSpec::new(
            "later",
            "later",
            "needs a runner",
            Resource::Cpu,
        ));
        jobs.shutdown();
        (done, waiting)
    };
    let jobs = JobSystem::start(config()).unwrap();
    assert_eq!(
        jobs.get(done).unwrap().state,
        JobState::Succeeded,
        "history kept"
    );
    let restored = jobs.get(waiting).unwrap();
    assert_eq!(restored.state, JobState::Queued);
    assert!(restored.resumed);
    jobs.register("later", |_: &JobContext, _: &JobSpec| {
        Ok(json!("picked up"))
    });
    assert_eq!(
        jobs.wait(waiting, T).unwrap().output,
        Some(json!("picked up"))
    );
    let next = jobs.submit(JobSpec::new("ok", "new", "new", Resource::Cpu));
    assert!(next > waiting, "ids are not reused across restarts");
    let lines = std::fs::read_to_string(&journal).unwrap().lines().count();
    assert!(lines >= 3);
    jobs.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn listeners_see_every_transition() {
    let jobs = system(1);
    let seen = Arc::new(Mutex::new(Vec::new()));
    let s = seen.clone();
    jobs.subscribe(move |r| s.lock().unwrap().push(r.state));
    jobs.register("ok", |ctx: &JobContext, _: &JobSpec| {
        ctx.progress(0.5, "half");
        Ok(json!(null))
    });
    let id = jobs.submit(JobSpec::new("ok", "ok", "ok", Resource::Cpu));
    jobs.wait(id, T);
    std::thread::sleep(Duration::from_millis(20));
    let states = seen.lock().unwrap().clone();
    assert_eq!(states.first(), Some(&JobState::Queued));
    assert!(states.contains(&JobState::Running));
    assert_eq!(states.last(), Some(&JobState::Succeeded));
}
