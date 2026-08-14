use std::time::Duration;

/// Whether `pid` still exists, without waiting on it or signalling it.
///
/// `kill(pid, 0)` and not `ps`: `ps` output is intermittently truncated under
/// the harness this tier runs in -- `ps aux | wc -l` returned 31 on a machine
/// with 830 processes -- and a truncated `ps` reads as "the process is gone",
/// which is the exact false pass this test exists to prevent.
fn alive(pid: u32) -> bool {
    let pid = rustix::process::Pid::from_raw(
        i32::try_from(pid).expect("a pid fits in the type the kernel gives it in"),
    )
    .expect("a spawned process has a non-zero pid");
    rustix::process::test_kill_process(pid).is_ok()
}

/// The engine must die when the process that started it does, and
/// `kill_on_drop(true)` does not deliver that.
///
/// **This is a regression test for a real leak.** An `arca-engine` was found on
/// this machine still running four days after the live run that spawned it,
/// orphaned to PID 1. `kill_on_drop` runs on drop, and a `SIGKILL`ed test
/// binary drops nothing. Task 13 multiplies engine starts, so the tier now
/// spawns through `common::supervised`, whose watcher kills the child when this
/// process stops holding the write end of the pipe.
///
/// **Closing the pipe IS the fault being simulated, not a stand-in for it.**
/// What a `SIGKILL`ed parent does to the child's stdin is exactly this: the
/// kernel closes the last write end. So the mechanism under test is driven by
/// its real trigger, and nothing here has to kill a test process to reach it.
///
/// **What this does NOT prove.** It says nothing about the wrapper itself being
/// killed, or about a `kill -9` to the whole process group -- both leak the
/// child exactly as before, and both are recorded on `SUPERVISOR`. It also
/// drives `/bin/sleep` rather than `arca-engine`, deliberately: the property is
/// the supervisor's and the substitution keeps this test out of `--ignored`,
/// where the tier's engine tests have to live and where CI never reaches them.
///
/// **SEEN TO FAIL under three separate mutations, each on the mechanism and
/// not on this file:**
///
/// - the watcher line deleted from `SUPERVISOR` entirely: `the supervised child
///   outlived the pipe: 10790 still running 5s after it closed`, and pid 10790
///   was then found in `ps aux` orphaned -- the leak itself, reproduced;
/// - `.stdin` left inherited rather than piped, so dropping it closes nothing:
///   the same message, and the wrapper survived too;
/// - the watcher's `<&3` removed so it reads the `/dev/null` a background
///   command is given: `the supervisor started no child within 10s`, because
///   the child was killed before `pgrep` could ever see it.
#[tokio::test]
async fn a_supervised_child_dies_when_its_parent_stops_holding_the_pipe() {
    let mut child = crate::common::supervised("/bin/sleep", &["600"])
        .spawn()
        .expect("the supervisor spawns");

    // The wrapper backgrounds the child and the watcher before it blocks, so
    // wait for the grandchild to actually exist rather than racing the shell.
    // `pgrep -P` reads the wrapper's children; the sleep is one of them.
    let sleep = grandchild_of(child.id().expect("a running supervisor")).await;
    assert!(
        alive(sleep),
        "the supervised child must be running: {sleep}"
    );

    // Exactly what a SIGKILLed parent does, and the only thing it does.
    drop(child.stdin.take());

    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while alive(sleep) && std::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        !alive(sleep),
        "the supervised child outlived the pipe: {sleep} still running 5s after it closed"
    );

    // And the wrapper must not linger either: an `sh` per leaked engine is the
    // same leak in a cheaper process.
    let stopped = tokio::time::timeout(Duration::from_secs(5), child.wait()).await;
    assert!(
        stopped.is_ok(),
        "the supervisor must exit once its child is gone"
    );
}

/// The pid of the `sleep` the wrapper started, waited for rather than assumed.
async fn grandchild_of(supervisor: u32) -> u32 {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        let output = tokio::process::Command::new("/usr/bin/pgrep")
            .arg("-P")
            .arg(supervisor.to_string())
            .arg("sleep")
            .output()
            .await
            .expect("pgrep runs");
        let found = String::from_utf8_lossy(&output.stdout)
            .lines()
            .next()
            .and_then(|line| line.trim().parse::<u32>().ok());
        if let Some(pid) = found {
            return pid;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the supervisor started no child within 10s"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}
