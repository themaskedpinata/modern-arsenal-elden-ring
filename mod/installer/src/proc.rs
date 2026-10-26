//! Runs the build's external programs (Python scripts, which run the Unlinker / WitchyBND): no window, a timeout,
//! a cancel flag, stdout + stderr merged line by line into `<root>\logs\install\<name>.log` and the progress
//! callback. Never used for games, Steam, Seamless or me3 (the launcher spawns me3 itself, at play time only).

use std::io::{BufRead, BufReader, Write};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[derive(Clone, Debug)]
pub struct Job {
    pub name: String,
    pub exe: PathBuf,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub cwd: PathBuf,
    pub timeout: Duration,
    pub log: PathBuf,
}

#[derive(Clone, Debug, PartialEq)]
pub struct JobResult {
    pub code: Option<i32>,
    pub timed_out: bool,
    /// Stopped by the cancel flag (the whole process tree was killed).
    pub cancelled: bool,
    pub secs: f64,
    /// The last non-empty output line (for the one-line status).
    pub last_line: String,
}

impl JobResult {
    pub fn ok(&self) -> bool {
        self.code == Some(0) && !self.timed_out && !self.cancelled
    }
}

/// Runs the job, calling `line` for every output line. The log file is written even on failure.
pub fn run(job: &Job, line: impl FnMut(&str)) -> std::io::Result<JobResult> {
    run_cancellable(job, None, line)
}

/// [`run`], also stopped (the process tree killed, within ~50 ms) when `cancel` becomes true.
pub fn run_cancellable(job: &Job, cancel: Option<&AtomicBool>, mut line: impl FnMut(&str)) -> std::io::Result<JobResult> {
    if let Some(d) = job.log.parent() {
        std::fs::create_dir_all(d)?;
    }
    let mut log = std::fs::File::create(&job.log)?;
    writeln!(log, "# {} {}", job.exe.display(), job.args.join(" "))?;
    let t0 = Instant::now();
    let mut cmd = Command::new(&job.exe);
    cmd.args(&job.args)
        .current_dir(&job.cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(CREATE_NO_WINDOW);
    for (k, v) in &job.env {
        cmd.env(k, v);
    }
    let mut child = cmd.spawn()?;
    // the whole process tree (python -> Unlinker / WitchyBND) lives in a job object: a timeout or this program's
    // exit kills all of it
    let tree = KillTree::new(&child);
    let (tx, rx) = mpsc::channel::<String>();
    let mut readers = Vec::new();
    if let Some(o) = child.stdout.take() {
        let tx = tx.clone();
        readers.push(std::thread::spawn(move || {
            for l in BufReader::new(o).split(b'\n').map_while(Result::ok) {
                let _ = tx.send(String::from_utf8_lossy(&l).trim_end_matches('\r').to_string());
            }
        }));
    }
    if let Some(e) = child.stderr.take() {
        let tx = tx.clone();
        readers.push(std::thread::spawn(move || {
            for l in BufReader::new(e).split(b'\n').map_while(Result::ok) {
                let _ = tx.send(String::from_utf8_lossy(&l).trim_end_matches('\r').to_string());
            }
        }));
    }
    drop(tx);
    let mut last = String::new();
    let mut timed_out = false;
    let mut cancelled = false;
    let mut handle = |l: String, log: &mut std::fs::File, last: &mut String| {
        let _ = writeln!(log, "{l}");
        if !l.trim().is_empty() {
            *last = l.trim().to_string();
        }
        line(&l);
    };
    let code = loop {
        while let Ok(l) = rx.recv_timeout(Duration::from_millis(50)) {
            handle(l, &mut log, &mut last);
        }
        if let Some(st) = child.try_wait()? {
            break st.code();
        }
        let stop = cancel.is_some_and(|c| c.load(Ordering::SeqCst));
        if stop || t0.elapsed() > job.timeout {
            tree.kill();
            let _ = child.kill();
            let _ = child.wait();
            if stop {
                cancelled = true;
            } else {
                timed_out = true;
            }
            break None;
        }
    };
    drop(tree);
    if !timed_out && !cancelled {
        for r in readers {
            let _ = r.join();
        }
    }
    while let Ok(l) = rx.try_recv() {
        handle(l, &mut log, &mut last);
    }
    let secs = t0.elapsed().as_secs_f64();
    let why = if timed_out {
        " (TIMEOUT, killed)"
    } else if cancelled {
        " (CANCELLED, killed)"
    } else {
        ""
    };
    writeln!(log, "# exit {code:?}{why} after {secs:.1} s")?;
    Ok(JobResult { code, timed_out, cancelled, secs, last_line: last })
}

/// A Win32 job object with kill-on-close holding the child (best effort: without it only the child is killed).
struct KillTree(Option<windows::Win32::Foundation::HANDLE>);

impl KillTree {
    fn new(child: &std::process::Child) -> KillTree {
        use std::os::windows::io::AsRawHandle;
        use windows::Win32::Foundation::{CloseHandle, HANDLE};
        use windows::Win32::System::JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation, SetInformationJobObject,
        };
        // SAFETY: a fresh job handle; the info struct is valid for its size; the process handle is the child's.
        unsafe {
            let Ok(job) = CreateJobObjectW(None, windows::core::PCWSTR::null()) else { return KillTree(None) };
            let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let ok = SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const _,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
            .is_ok()
                && AssignProcessToJobObject(job, HANDLE(child.as_raw_handle())).is_ok();
            if !ok {
                let _ = CloseHandle(job);
                return KillTree(None);
            }
            KillTree(Some(job))
        }
    }

    fn kill(&self) {
        if let Some(j) = self.0 {
            // SAFETY: our own job handle.
            unsafe {
                let _ = windows::Win32::System::JobObjects::TerminateJobObject(j, 1);
            }
        }
    }
}

impl Drop for KillTree {
    fn drop(&mut self) {
        if let Some(j) = self.0.take() {
            // SAFETY: our own job handle; kill-on-close ends anything still running in it.
            unsafe {
                let _ = windows::Win32::Foundation::CloseHandle(j);
            }
        }
    }
}

/// A job for `python -I <script> <args>` with the build's environment.
pub fn python_job(name: &str, python: &Path, script: &Path, args: &[String], env: &[(String, String)], cwd: &Path, minutes: u64, log_dir: &Path) -> Job {
    let mut a = vec!["-I".to_string(), script.display().to_string()];
    a.extend_from_slice(args);
    Job {
        name: name.to_string(),
        exe: python.to_path_buf(),
        args: a,
        env: env.to_vec(),
        cwd: cwd.to_path_buf(),
        timeout: Duration::from_secs(minutes * 60),
        log: log_dir.join(format!("{name}.log")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::testutil::TempDir;

    fn cmd_job(t: &TempDir, script: &str, timeout: Duration) -> Job {
        Job {
            name: "t".into(),
            exe: PathBuf::from(std::env::var("ComSpec").unwrap_or_else(|_| r"C:\Windows\System32\cmd.exe".into())),
            args: vec!["/d".into(), "/c".into(), script.into()],
            env: vec![("MW2ER_TEST_VAR".into(), "hello".into())],
            cwd: t.0.clone(),
            timeout,
            log: t.0.join(r"logs\t.log"),
        }
    }

    #[test]
    fn captures_output_exit_code_and_env() {
        let t = TempDir::new("proc");
        let mut lines = Vec::new();
        let r = run(&cmd_job(&t, "echo one& echo %MW2ER_TEST_VAR% 1>&2& exit 3", Duration::from_secs(30)), |l| lines.push(l.to_string())).unwrap();
        assert_eq!(r.code, Some(3));
        assert!(!r.ok());
        assert!(lines.iter().any(|l| l.trim() == "one") && lines.iter().any(|l| l.trim() == "hello"), "{lines:?}");
        let log = std::fs::read_to_string(t.0.join(r"logs\t.log")).unwrap();
        assert!(log.contains("one") && log.contains("# exit Some(3)"));
    }

    #[test]
    fn kills_on_timeout() {
        let t = TempDir::new("proc-to");
        let r = run(&cmd_job(&t, "ping -n 30 127.0.0.1 >nul", Duration::from_millis(500)), |_| {}).unwrap();
        assert!(r.timed_out && !r.ok());
        assert!(r.secs < 20.0);
    }

    #[test]
    fn cancel_kills_the_tree() {
        let t = TempDir::new("proc-cancel");
        let flag = std::sync::Arc::new(AtomicBool::new(false));
        let f2 = flag.clone();
        let setter = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(400));
            f2.store(true, Ordering::SeqCst);
        });
        let r = run_cancellable(&cmd_job(&t, "echo started& ping -n 60 127.0.0.1 >nul", Duration::from_secs(120)), Some(&flag), |_| {}).unwrap();
        setter.join().unwrap();
        assert!(r.cancelled && !r.timed_out && !r.ok());
        assert!(r.secs < 20.0, "{}", r.secs);
        assert!(std::fs::read_to_string(t.0.join(r"logs\t.log")).unwrap().contains("CANCELLED"));
    }
}
