use crate::common::{
    LiveEngine, await_state, base_oci_layout, layout_running, policy_request_from_manifest,
};
use camino::Utf8Path;
use gascan_arca::ArcaBackend;
use gascan_core::runtime::{ContainerState, RuntimeBackend};
use gascan_core::sandbox::SandboxId;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// The backend over a real engine, not a fake.
async fn backend(engine: &LiveEngine) -> ArcaBackend<gascan_arca::ChannelTransport> {
    ArcaBackend::new(engine.transport().await)
}

/// The tag the derived layout is loaded under.
const TAG: &str = "gascan-live-logs:latest";

/// `user = 'root'` because the base layout is a stock alpine with no
/// `workspace` user, exactly as `lifecycle.rs` records.
const MANIFEST: &str = "version = 1\nnetwork = 'networked'\nuser = 'root'\n";

/// What the guest prints, in the order it prints it.
///
/// **Odd lines go to stderr and even lines to stdout, which is the whole point
/// of the list.** The engine keeps a `stdout.log` and a `stderr.log` as well as
/// a `combined.log`, and only the combined file records the order the two
/// streams actually arrived in. A `Logs` that read one stream file returns half
/// of this; a `Logs` that read both and concatenated them returns all of it in
/// the wrong order. Only reading the combined file returns this list.
const LINES: [&str; 4] = ["line-0", "line-1", "line-2", "line-3"];

/// The whole log as the consumer must receive it.
fn whole_log() -> String {
    suffix_from(0)
}

/// The log from `index` onward, which is what a `since` at that line's stamp
/// must return.
fn suffix_from(index: usize) -> String {
    LINES[index..]
        .iter()
        .map(|line| format!("{line}\n"))
        .collect()
}

/// Host milliseconds, on the same clock the engine stamps entries with -- the
/// entries are stamped by the engine's own writer as it receives the guest's
/// output, so this process and that writer read one clock.
fn now_millis() -> i64 {
    i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("a clock after 1970")
            .as_millis(),
    )
    .expect("a millisecond count that fits an i64")
}

/// `Logs` over a real container: both streams, in order, and a
/// `since_unix_millis` that cuts between two entries **inside a single
/// wall-clock second**.
///
/// **The sub-second cut is the reason this test exists in this form.**
/// `LogsRequest.since_unix_millis` is milliseconds, and Arca's log writer
/// stamped its entries with an `ISO8601DateFormatter` at default options, which
/// emits whole seconds. Under that stamp every entry in a second reads as that
/// second's first instant, so no `since` can separate two entries written
/// within one second: the request returns all of them or none of them. Arca's
/// writer gained `.withFractionalSeconds` for this task, and this is the
/// instrument that says so from outside the engine.
///
/// **The stamps are read out of the engine rather than guessed at.** This
/// process cannot see an entry's `time` -- `LogsChunk` carries bytes and
/// nothing else -- so [`stamp_of`] bisects `since` to find, for each line, the
/// largest value that still returns it. That value IS the entry's stamp, to the
/// millisecond, because the filter is inclusive. Nothing here depends on
/// guest/host clock agreement or on how long a boot took.
///
/// The four lines are 250ms apart, so their span is 750ms and at most one
/// second boundary can fall inside it: at least one adjacent pair is guaranteed
/// to sit within a single second, and that pair is the one the cut is taken
/// from.
///
/// **What this does NOT prove.** Nothing about follow mode, which the contract
/// forbids and this engine does not have. Nothing about a log large enough to
/// need more than one chunk -- four short lines are one frame, and where the
/// engine splits is asserted in Arca's own `LogsTests`. And it says nothing
/// about `Exec`, which `exec.rs` covers -- no method in this build answers
/// `unsupported_capability` any more; see `read_rpcs.rs`.
///
/// RUN, against Arca `8679113` on 2026-08-16: the full live tier reported
/// `20 passed; 0 failed` in 234.34s, this test among them. An earlier version
/// of this paragraph said the test had never been executed, and was left
/// standing after the run that executed it.
#[tokio::test]
#[ignore = "requires a built arca-engine named by GASCAN_ARCA_ENGINE_BIN, a kernel, a vminit \
            layout and a base OCI layout"]
async fn logs_returns_both_streams_in_order_and_since_millis_cuts_inside_one_second() {
    let images = tempfile::tempdir().expect("a temporary layout root");
    let layout = layout_running(
        &base_oci_layout(),
        Utf8Path::from_path(images.path()).expect("a utf-8 path"),
        TAG,
        &[
            "sh",
            "-c",
            // Fractional `sleep` is busybox's, which the base layout is. The
            // gaps are what put the four stamps in distinct milliseconds; the
            // `1>&2` on the odd lines is what makes the order load-bearing.
            "echo line-0; sleep 0.25; echo line-1 1>&2; sleep 0.25; \
             echo line-2; sleep 0.25; echo line-3 1>&2",
        ],
    );

    let engine = LiveEngine::start_with_images(&[&layout]).await;
    let backend = backend(&engine).await;
    let (_root, request) = policy_request_from_manifest("logs", &engine.image(TAG), MANIFEST);

    backend
        .prepare_image(request.image())
        .await
        .expect("the store holds the image the request names");
    backend
        .create(request.clone())
        .await
        .expect("create against a seeded store must succeed");

    // Before the container can have written anything, so it bounds the search
    // below from underneath.
    let before = now_millis();

    backend
        .start(request.id())
        .await
        .expect("start must boot the sandbox");
    // Generous: a first start boots a virtual machine from a kernel and a
    // vminit layout. Bounded all the same.
    await_state(
        &backend,
        &request,
        ContainerState::Running,
        Duration::from_secs(180),
    )
    .await;

    let complete = await_complete_log(&backend, request.id(), Duration::from_secs(180)).await;
    // After the whole log was observed, so it bounds the search from above.
    let after = now_millis() + 1;

    assert_eq!(
        complete,
        whole_log(),
        "Logs must return both streams interleaved in the order the guest wrote them"
    );

    // Each line's stamp, to the millisecond, read out of the engine.
    let mut stamps = Vec::new();
    for index in 0..LINES.len() {
        stamps.push(stamp_of(&backend, request.id(), index, before, after).await);
    }
    assert!(
        stamps.windows(2).all(|pair| pair[0] < pair[1]),
        "each entry must carry its own stamp; the four were {stamps:?}"
    );

    // The adjacent pair that sits inside one wall-clock second. Guaranteed to
    // exist: the four lines span 750ms, so at most one second boundary can fall
    // among them.
    let inside = stamps
        .windows(2)
        .position(|pair| pair[0] / 1000 == pair[1] / 1000)
        .unwrap_or_else(|| {
            panic!(
                "no two of the four entries landed in one second, so this test \
                 could not exercise a sub-second cut; the stamps were {stamps:?}"
            )
        });
    let earlier = stamps[inside];
    let cut = stamps[inside + 1];

    assert_eq!(
        logs(&backend, request.id(), Some(earlier)).await,
        suffix_from(inside),
        "a since at an entry's own stamp includes that entry"
    );
    assert_eq!(
        logs(&backend, request.id(), Some(cut)).await,
        suffix_from(inside + 1),
        "a since {}ms later, inside the same second, excludes it -- which a \
         whole-second stamp could not express",
        cut - earlier
    );
    assert_eq!(
        logs(&backend, request.id(), Some(stamps[LINES.len() - 1] + 1)).await,
        "",
        "a since one millisecond past the last entry excludes the whole log"
    );

    // No `stop` and no `remove`. This container's PID 1 has already exited, and
    // `lifecycle.rs` records that stopping one in that state fails with
    // `CancellationError()` while `Remove` refuses it as `invalid_state`
    // regardless, because the refusal reads the persisted state and nothing
    // writes the guest's exit back to it. The engine's state root is a
    // `TempDir` this fixture owns, so there is nothing left behind to clean.
    engine.kill().await;
}

/// The log as one string, or a panic naming what came back instead.
async fn logs(
    backend: &ArcaBackend<gascan_arca::ChannelTransport>,
    id: &SandboxId,
    since_millis: Option<i64>,
) -> String {
    let bytes = backend
        .logs(id, since_millis)
        .await
        .unwrap_or_else(|error| panic!("logs({since_millis:?}) must answer: {error}"));
    String::from_utf8(bytes)
        .unwrap_or_else(|error| panic!("the log must be the utf-8 the guest printed: {error}"))
}

/// Polls until the guest's last line is in the log.
///
/// The container prints for three quarters of a second and exits, and nothing
/// tells this process when it has finished, so the last line is the sentinel.
/// Returns the whole log as it stood when that line appeared, which is what the
/// caller compares against the expected order.
async fn await_complete_log(
    backend: &ArcaBackend<gascan_arca::ChannelTransport>,
    id: &SandboxId,
    bound: Duration,
) -> String {
    let sentinel = format!("{}\n", LINES[LINES.len() - 1]);
    let started = std::time::Instant::now();
    let mut last = String::new();
    while started.elapsed() < bound {
        last = logs(backend, id, None).await;
        if last.contains(&sentinel) {
            return last;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    panic!(
        "the guest's last line never reached the log within {:.1}s; \
         it last held {last:?}",
        bound.as_secs_f64()
    );
}

/// The millisecond `LINES[index]` is stamped with, bisected out of the engine.
///
/// `since_unix_millis` is inclusive, so the largest value that still returns a
/// line is exactly that line's stamp. `included` must be a value that returns
/// it and `excluded` one that does not; both bounds are asserted before the
/// search, because a bisection over a broken invariant answers confidently and
/// wrongly.
async fn stamp_of(
    backend: &ArcaBackend<gascan_arca::ChannelTransport>,
    id: &SandboxId,
    index: usize,
    included: i64,
    excluded: i64,
) -> i64 {
    let line = LINES[index];
    let mut low = included;
    let mut high = excluded;
    assert!(
        logs(backend, id, Some(low)).await.contains(line),
        "the search's lower bound {low} must still return {line}"
    );
    assert!(
        !logs(backend, id, Some(high)).await.contains(line),
        "the search's upper bound {high} must already exclude {line}"
    );
    while high - low > 1 {
        let mid = low + (high - low) / 2;
        if logs(backend, id, Some(mid)).await.contains(line) {
            low = mid;
        } else {
            high = mid;
        }
    }
    low
}
