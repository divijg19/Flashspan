use log::{error, info, warn};
use rodio::{Decoder, DeviceSinkBuilder, Player};
use std::io::Cursor;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Mutex, MutexGuard, OnceLock, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

/// Cut semantics: every new sound first drops anything still queued or
/// playing, then starts. Rationale: beeps are time-critical flash ticks
/// (100ms exposure vs ~110ms beep asset); queueing would lag behind the
/// visuals and overlap subsequent ticks.
///
/// Deliberately not `Player::clear()`/`stop()` on this path: `clear()`
/// blocks until the current sound ends, and `append()` after `stop()` can
/// block while the queue flushes (rodio 0.23). `skip_one()` never blocks.
#[derive(Debug, Clone, Copy)]
enum AudioCommand {
    /// Open the output device without making sound, ignoring any retry
    /// cooldown left over from an earlier failure. Inaudible by construction:
    /// it carries nothing to play and never reaches the sink.
    Warmup,
    /// Flash tick: cut previous audio, then play the beep asset.
    Beep,
    /// Validation feedback: cut previous audio, then play the payload.
    Feedback(&'static [u8]),
    /// Drop everything; used when a session stops.
    Silence,
}

/// Minimal sink surface so command handling is unit-testable without audio
/// hardware. All calls happen on the single audio worker thread.
trait AudioSink {
    fn queued(&self) -> usize;
    fn skip_one(&self);
    fn append_bytes(&self, data: &'static [u8]);
}

impl AudioSink for Player {
    fn queued(&self) -> usize {
        self.len()
    }

    fn skip_one(&self) {
        Self::skip_one(self);
    }

    fn append_bytes(&self, data: &'static [u8]) {
        match Decoder::try_from(Cursor::new(data)) {
            Ok(src) => self.append(src),
            Err(e) => error!("audio decode error: {e}"),
        }
    }
}

/// Whether a cut finished, or the sink stopped consuming half way.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CutOutcome {
    /// Nothing is left playing; a new sound can start immediately.
    Drained,
    /// The queue never emptied. On a real sink that means the output stream
    /// stopped pulling samples, so no further sound is possible until the
    /// device is reopened.
    Stalled,
}

/// How long to wait for the audio callback to apply a scheduled skip.
///
/// rodio applies a skip from inside the stream callback (`periodic_access`,
/// every 5ms), so the queue count falls asynchronously. The queue holds at most
/// one sound — every append cuts first — so this is roughly twenty times the
/// observed latency: enough that a merely busy callback is never mistaken for a
/// dead device, and short enough that a dead one is found within a single flash
/// tick rather than at the next session.
const CUT_DEADLINE: Duration = Duration::from_millis(100);

/// How often to look, while waiting for a cut to land. Sleeping rather than
/// spinning is the whole point: a tight `while queued() > 0` loop burns a core
/// between every pair of beeps, and never returns at all once the stream is
/// gone.
const CUT_POLL: Duration = Duration::from_millis(1);

/// Cut everything queued, then wait for the sink to confirm the cut landed.
///
/// The skip is scheduled once per sound that is in the queue *now* — rodio
/// guards `skip_one` internally with `len > to_clear`, so asking again while
/// nothing has moved would be exactly the busy-wait this replaces. Waiting
/// afterwards is what keeps the cut correct: appending while a skip is still
/// pending lets that pending skip land on the sound being appended, and a beep
/// silently swallowed every so often is the whole bug this function exists to
/// avoid.
fn cut(player: &impl AudioSink) -> CutOutcome {
    let queued = player.queued();
    for _ in 0..queued {
        player.skip_one();
    }

    let deadline = Instant::now() + CUT_DEADLINE;
    while player.queued() > 0 {
        if Instant::now() >= deadline {
            return CutOutcome::Stalled;
        }
        thread::sleep(CUT_POLL);
    }
    CutOutcome::Drained
}

/// Cut whatever is playing, then queue `data` — unless the sink has stalled, in
/// which case queueing it would only pile sound onto a stream that has stopped.
fn cut_then_append(player: &impl AudioSink, data: &'static [u8]) -> CutOutcome {
    match cut(player) {
        CutOutcome::Drained => {
            player.append_bytes(data);
            CutOutcome::Drained
        }
        stalled @ CutOutcome::Stalled => stalled,
    }
}

/// Apply one command to an open sink, reporting whether that sink is still fit
/// to take another. The worker tears the device down on `Stalled`.
///
/// `AudioCommand::Warmup` reaches the sink as a no-op: it is about the device,
/// not the queue, and the worker spends it on an open instead.
fn handle_command(player: &impl AudioSink, command: AudioCommand) -> CutOutcome {
    match command {
        AudioCommand::Warmup => CutOutcome::Drained,
        AudioCommand::Silence => cut(player),
        AudioCommand::Beep => cut_then_append(player, include_bytes!("../../src/assets/beep.wav")),
        AudioCommand::Feedback(data) => cut_then_append(player, data),
    }
}

/// The output device's verdict, written by the worker whenever it opens one,
/// fails to open one, or loses an open stream.
///
/// Deliberately updatable rather than a `OnceLock`: a device that is missing
/// when the first command arrives and appears a second later — the audio server
/// still starting, a headset mid-transition — has to move the status line when
/// it does, instead of the first answer being latched for the life of the
/// process. `None` means the worker has not reached a verdict yet, which is not
/// a failure.
struct DeviceState {
    outcome: Mutex<Option<Result<(), String>>>,
}

impl DeviceState {
    const fn new() -> Self {
        Self {
            outcome: Mutex::new(None),
        }
    }

    /// Publish a verdict, reporting whether it *changed*. The worker logs the
    /// transition rather than the attempt: a retry happens every second for as
    /// long as a device is missing, and restating the same dead device is noise.
    fn record(&self, outcome: Result<(), String>) -> bool {
        let mut cell = self.lock();
        if *cell == Some(outcome.clone()) {
            return false;
        }
        *cell = Some(outcome);
        true
    }

    fn get(&self) -> Option<Result<(), String>> {
        self.lock().clone()
    }

    /// A poisoned lock still holds the last verdict, every write here replaces
    /// the value wholesale, and session lifecycle must never depend on audio
    /// hardware — including on not panicking because a reader did.
    fn lock(&self) -> MutexGuard<'_, Option<Result<(), String>>> {
        self.outcome.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The worker's answer to "is there a device to play into?".
enum Opened<S> {
    /// Taking commands, carrying the sink handle and the player.
    Ready((S, Player)),
    /// The open failed; the verdict is already on the status line.
    Failed,
    /// Too soon after a failure to enumerate output devices again.
    CoolingDown,
}

/// How long to wait before re-attempting an output device that just failed.
const RETRY_COOLDOWN: Duration = Duration::from_secs(1);

/// Make sure there is a live sink, opening one if there is not yet.
///
/// `force` skips the cooldown, which is what `AudioCommand::Warmup` passes: a
/// session starting is the moment worth paying an enumeration for. Everything
/// else is throttled, because a flash tick arrives every ~110ms and
/// re-enumerating output devices at that rate on a machine that has none would
/// burn a core for the length of the session.
fn ensure_open<S>(
    open: &impl Fn() -> Result<(S, Player), String>,
    device: &DeviceState,
    retry_at: &mut Option<Instant>,
    retry_after: Duration,
    force: bool,
) -> Opened<S> {
    if let Some(next) = *retry_at
        && !force
        && Instant::now() < next
    {
        return Opened::CoolingDown;
    }

    match open() {
        Ok(opened) => {
            *retry_at = None;
            if device.record(Ok(())) {
                info!("audio worker initialized output sink");
            }
            Opened::Ready(opened)
        }
        Err(err) => {
            if device.record(Err(STATUS_NO_OUTPUT_DEVICE.to_string())) {
                error!("audio worker has no usable output device: {err}");
            }
            *retry_at = Some(Instant::now() + retry_after);
            Opened::Failed
        }
    }
}

/// The audio worker's body: keep a device open, and serve commands to it.
///
/// Split out of the thread spawn so the failure and recovery paths are
/// reachable from a test with no audio hardware to fail with: `open` is the real
/// device in production and any closure in a test.
///
/// The worker never exits on a fault. A device that fails to open would
/// otherwise be dead for the life of the process — the sender half of this
/// channel is never replaced — so a command that needs a device asks for one
/// again once its cooldown has passed, and sound returns on its own.
///
/// The driver's own error text goes to the log; `device` carries only the stable
/// token the status line renders.
fn run_worker<S>(
    rx: &Receiver<AudioCommand>,
    device: &DeviceState,
    open: &impl Fn() -> Result<(S, Player), String>,
    retry_after: Duration,
) {
    let mut sink: Option<(S, Player)> = None;
    let mut retry_at: Option<Instant> = None;

    while let Ok(command) = rx.recv() {
        let opening = matches!(command, AudioCommand::Warmup);

        if sink.is_none() {
            // A cut with no sink open has nothing to cut, and opening a device
            // purely to prove it silent would charge a cold open for stopping a
            // session that never made a sound.
            if !opening && matches!(command, AudioCommand::Silence) {
                continue;
            }
            match ensure_open(open, device, &mut retry_at, retry_after, opening) {
                Opened::Ready(pair) => sink = Some(pair),
                Opened::Failed | Opened::CoolingDown => continue,
            }
        }

        // Warmup's whole payload is the open above; it plays nothing.
        if opening {
            continue;
        }

        let outcome = {
            let Some((_, player)) = sink.as_ref() else {
                continue;
            };
            handle_command(player, command)
        };

        if let CutOutcome::Stalled = outcome {
            // The stream stopped pulling. Nothing queued behind it will ever be
            // heard, so drop the handle now and let a later command reopen,
            // reporting the transition once rather than on every following tick.
            sink = None;
            retry_at = Some(Instant::now() + retry_after);
            if device.record(Err(STATUS_NO_OUTPUT_DEVICE.to_string())) {
                warn!("audio output stream stalled; reopening the device");
            }
        }
    }
    info!("audio worker receiver loop ended");
}

static AUDIO_SENDER: OnceLock<Result<Sender<AudioCommand>, String>> = OnceLock::new();

static AUDIO_DEVICE: DeviceState = DeviceState::new();

/// Set once a playback failure has been reported. A machine with no output
/// device fails every flash tick, and a session is hundreds of ticks, so the
/// failure is written to the log once rather than once per beep. Released again
/// by `clear_play_failure_latch` the next time a send lands, so a fault that
/// follows a recovery is a new episode rather than one already told.
static AUDIO_FAILURE_REPORTED: AtomicBool = AtomicBool::new(false);

fn get_audio_sender() -> Result<&'static Sender<AudioCommand>, String> {
    let value = AUDIO_SENDER.get_or_init(|| {
        let (tx, rx) = channel::<AudioCommand>();

        match std::thread::Builder::new()
            .name("audio-worker".into())
            .spawn(move || {
                run_worker(
                    &rx,
                    &AUDIO_DEVICE,
                    &|| {
                        // The sink handle has to travel back with the player:
                        // it owns the cpal stream, and dropping it stops
                        // playback dead while every status read still says the
                        // device is ready.
                        DeviceSinkBuilder::open_default_sink()
                            .map(|sink| {
                                let player = Player::connect_new(sink.mixer());
                                (sink, player)
                            })
                            .map_err(|err| err.to_string())
                    },
                    RETRY_COOLDOWN,
                );
            })
            .map_err(|err| err.to_string())
        {
            Ok(_) => Ok(tx),
            Err(err) => Err(err),
        }
    });
    value.as_ref().map_err(std::clone::Clone::clone)
}

fn send_command(command: AudioCommand) -> Result<(), String> {
    let sender = match get_audio_sender() {
        Ok(s) => s,
        Err(e) => {
            warn!("audio sender init failed: {e}");
            return Err(e);
        }
    };

    sender
        .send(command)
        .map_err(|e| format!("audio send error: {e}"))
}

fn command_for_kind(kind: &str) -> Result<AudioCommand, String> {
    match kind {
        "beep" => Ok(AudioCommand::Beep),
        "applause" => Ok(AudioCommand::Feedback(include_bytes!(
            "../../src/assets/applause.wav"
        ))),
        "buzzer" => Ok(AudioCommand::Feedback(include_bytes!(
            "../../src/assets/buzzer.wav"
        ))),
        _ => Err("unknown sound kind".to_string()),
    }
}

/// Play a sound from Rust: the session's flash beeps and the answer feedback
/// both come through here, and nothing else can ask for one.
///
/// Deliberately not a `#[tauri::command]`. Rust owns playback on desktop, so
/// the UI has no "play a sound" request — the browser runtime has no such
/// member either — and the command that used to expose this had no caller. It
/// did carry an HTML Audio fallback that fetched all three clips at module load
/// for a path that could not run.
pub fn play_kind(kind: &str) -> Result<(), String> {
    if !is_enabled() {
        info!("sound disabled; skipping play_kind({kind})");
        return Ok(());
    }

    let command = match command_for_kind(kind) {
        Ok(command) => command,
        // An unknown kind is a caller bug rather than a device fault, so it is
        // always worth reporting.
        Err(e) => {
            error!("failed to play {kind}: {e}");
            return Err(e);
        }
    };

    match send_command(command) {
        Ok(()) => {
            clear_play_failure_latch(&AUDIO_FAILURE_REPORTED);
            Ok(())
        }
        Err(e) => {
            // Report only the first failure. A device that cannot play fails on
            // every flash tick, and a session is hundreds of ticks, so the log
            // would otherwise carry the same line for its whole length.
            report_play_failure_once(&AUDIO_FAILURE_REPORTED, kind, &e);
            Err(e)
        }
    }
}

/// Release the failure latch, so a fault that comes *after* a working send is
/// reported rather than swallowed as one already told.
///
/// The worker recovers by itself — a device that appears later is reopened on
/// the next command — so an unsettable latch would silence every failure for
/// the rest of the process, including a genuinely new one.
fn clear_play_failure_latch(latch: &AtomicBool) {
    latch.store(false, Ordering::SeqCst);
}

/// Claim `latch`, and report a playback failure only on the call that won it.
///
/// The latch is handed in rather than read from the process global so the
/// behaviour is testable: cargo runs tests in parallel, so a test that drove
/// `AUDIO_FAILURE_REPORTED` directly would race every other audio test and
/// leave the global latched for the rest of the process. This is the same seam
/// `status_with_state` uses, for the same reason.
///
/// Returns whether this call was the one that reported.
fn report_play_failure_once(latch: &AtomicBool, kind: &str, err: &str) -> bool {
    if latch.swap(true, Ordering::SeqCst) {
        // Another call already claimed it, and the log has this line.
        return false;
    }
    error!("failed to play {kind}: {err}");
    true
}

/// Drop any queued or playing audio. Best-effort: failures are ignored so
/// session lifecycle never depends on audio hardware. Skips entirely when
/// audio was never initialized, so stopping a silent session never pays
/// for a cold device open.
pub fn silence() {
    silence_with(AUDIO_SENDER.get());
}

/// The body of `silence`, with the worker's sender handed in rather than read
/// from the global. `AUDIO_SENDER` is a `OnceLock` and cargo runs tests in
/// parallel: a test that waited for another test to initialise it would be
/// racing, and one that initialised it itself would spawn a real audio worker
/// against whatever hardware the machine has.
fn silence_with(sender: Option<&Result<Sender<AudioCommand>, String>>) {
    // Nothing has tried to start audio, so there is nothing to cut, and
    // stopping a silent session must not pay for a cold device open.
    let Some(sender) = sender else {
        return;
    };
    let sender = match sender {
        Ok(sender) => sender,
        // A worker thread that never spawned cannot be cut either. This is the
        // same fault `play_kind` reports, so it still gets a line.
        Err(err) => {
            warn!("audio sender init failed: {err}");
            return;
        }
    };
    // Best-effort: session lifecycle never depends on audio hardware, so a
    // failed send is dropped rather than surfaced.
    let _ = sender.send(AudioCommand::Silence);
}

/// The warmup command. A variant of its own rather than a borrowed `Silence`,
/// because the two differ where it matters: a cut with no sink open has nothing
/// to cut and must not buy a device open, and a warmup exists precisely to buy
/// one. `handle_command` never spends it on sound.
const fn warmup_command() -> AudioCommand {
    AudioCommand::Warmup
}

/// Pre-warm the audio worker: forces thread spawn and output-device open
/// ahead of the first flash. Opening the default device involves device
/// enumeration and stream negotiation (hundreds of ms cold), which would
/// otherwise delay the first beep of the first session. Inaudible by
/// construction — it carries nothing to play. Never blocks the caller beyond
/// channel send and never panics: all failures are swallowed by `send_command`.
pub fn warmup() {
    let _ = send_command(warmup_command());
}

static SOUND_ENABLED: AtomicBool = AtomicBool::new(true);

pub fn set_enabled(v: bool) {
    SOUND_ENABLED.store(v, Ordering::SeqCst);
    if !v {
        // Muting has to cut what is already playing, not merely stop the next
        // sound: applause runs for seconds and outlives the click that mutes it.
        // `silence` costs nothing before anything has been initialised.
        silence();
    }
}

pub fn is_enabled() -> bool {
    SOUND_ENABLED.load(Ordering::SeqCst)
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct AudioStatus {
    pub enabled: bool,
    pub available: bool,
    pub detail: String,
}

/// The stable tokens the status line renders.
///
/// The raw backend error goes to the log instead of in here: the indicator is a
/// single reserved line, so a driver error string overran it, and no caller can
/// match on it. `STATUS_NOT_STARTED` and `STATUS_READY` are deliberately the
/// same words the browser runtime reports for the same two states, so the two
/// platforms describe themselves alike.
const STATUS_NOT_STARTED: &str = "not-started";
const STATUS_READY: &str = "ready";
const STATUS_NO_OUTPUT_DEVICE: &str = "no-output-device";
const STATUS_WORKER_FAILED: &str = "audio-worker-failed";

/// Read-only audio health for the sound status indicator. Never initializes
/// the device: an untouched worker reports available with detail
/// "not-started" (assume usable until proven otherwise).
///
/// The status derivation, separated from the process-global `OnceLock`s so the
/// whole state machine is testable. `AUDIO_SENDER` and `AUDIO_DEVICE` are both
/// `OnceLock`s and cargo runs tests in parallel: once any other test has
/// initialised one, its `None` branch is unreachable for the rest of the
/// process, so the real state machine needs this seam to be covered.
fn status_with_state(
    sender: Option<&Result<Sender<AudioCommand>, String>>,
    device: Option<&Result<(), String>>,
    enabled: bool,
) -> AudioStatus {
    let (available, detail) = match sender {
        // Nothing has tried to start audio yet.
        None => (true, STATUS_NOT_STARTED),
        // The worker thread itself never started.
        Some(Err(_)) => (false, STATUS_WORKER_FAILED),
        Some(Ok(_)) => match device {
            // The sink is open, so playback is possible.
            Some(Ok(())) => (true, STATUS_READY),
            // The sink never opened. `send_command` fails on this path, so this
            // is what "Sound ready" used to claim on a machine with no device.
            Some(Err(_)) => (false, STATUS_NO_OUTPUT_DEVICE),
            // The thread was spawned but has not reached the device yet.
            None => (true, STATUS_NOT_STARTED),
        },
    };
    AudioStatus {
        enabled,
        available,
        detail: detail.to_string(),
    }
}

pub fn status() -> AudioStatus {
    let device = AUDIO_DEVICE.get();
    status_with_state(AUDIO_SENDER.get(), device.as_ref(), is_enabled())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// All four branches of the status state machine, plus the enabled flag.
    /// These run against the extracted derivation rather than the globals, so
    /// the "nothing has tried audio yet" branch stays reachable despite cargo
    /// running tests in parallel against shared `OnceLock`s.
    #[test]
    fn status_reports_not_started_before_any_playback_attempt() {
        let no_sender: Option<&Result<Sender<AudioCommand>, String>> = None;
        let no_device: Option<&Result<(), String>> = None;
        let status = status_with_state(no_sender, no_device, true);
        assert!(status.enabled);
        assert!(status.available, "not-started is not a failure");
        assert_eq!(status.detail, STATUS_NOT_STARTED);
    }

    #[test]
    fn status_reports_ready_once_the_sink_is_open() {
        let sender = Ok(placeholder_sender());
        let status = status_with_state(Some(&sender), Some(&Ok(())), true);
        assert_eq!(status.detail, STATUS_READY);
        assert!(status.available);
    }

    /// The branch this whole seam exists for. `AUDIO_SENDER` becomes `Ok` the
    /// moment the worker thread spawns, well before the sink is negotiated, so
    /// an open failure has to be reported from the device slot. Without it the
    /// indicator claims "Sound ready" on a machine that can never play anything.
    #[test]
    fn status_reports_the_device_failure_when_the_sink_never_opened() {
        let sender = Ok(placeholder_sender());
        let dead = Err(STATUS_NO_OUTPUT_DEVICE.to_string());
        let status = status_with_state(Some(&sender), Some(&dead), true);
        assert!(!status.available, "a dead device is not 'available'");
        assert_eq!(status.detail, STATUS_NO_OUTPUT_DEVICE);
    }

    /// Muting must not be mistaken for a device failure, and a device failure
    /// must not be mistaken for a mute: the two flags are orthogonal.
    #[test]
    fn status_keeps_the_enabled_flag_and_the_device_independent() {
        let no_sender: Option<&Result<Sender<AudioCommand>, String>> = None;
        let no_device: Option<&Result<(), String>> = None;
        let sender = Ok(placeholder_sender());
        let no_worker: Result<Sender<AudioCommand>, String> = Err(STATUS_WORKER_FAILED.to_string());
        let open = Ok(());
        let dead = Err(STATUS_NO_OUTPUT_DEVICE.to_string());

        for (sender, device, expected_available, expected_detail) in [
            (no_sender, no_device, true, STATUS_NOT_STARTED),
            // The worker thread itself never spawned. That is a fault rather
            // than an idle state, and it is not a mute either.
            (Some(&no_worker), no_device, false, STATUS_WORKER_FAILED),
            (Some(&sender), no_device, true, STATUS_NOT_STARTED),
            (Some(&sender), Some(&open), true, STATUS_READY),
            (Some(&sender), Some(&dead), false, STATUS_NO_OUTPUT_DEVICE),
        ] {
            for enabled in [true, false] {
                let status = status_with_state(sender, device, enabled);
                assert_eq!(status.enabled, enabled);
                assert_eq!(status.available, expected_available);
                assert_eq!(status.detail, expected_detail);
            }
        }
    }

    /// End to end over the worker body rather than the derivation: this is the
    /// only way to make the device open fail deterministically, on a machine
    /// that may well have working audio. A failure must be recorded in a form
    /// the status line can report, the backend's own words must stay in the log
    /// instead of reaching the UI, and — unlike the loop this replaced — the
    /// worker must survive it: the sender half of the channel is never
    /// replaced, so a worker that returned here left the process mute until it
    /// was restarted.
    #[test]
    fn run_worker_records_a_device_that_never_opened() {
        let (tx, rx) = channel::<AudioCommand>();
        let device = DeviceState::new();

        // One command is all it takes: the worker records the failure and keeps
        // serving, so it is a closed channel — not a fault — that ends the call.
        tx.send(AudioCommand::Warmup).expect("channel is open");
        drop(tx);

        run_worker::<()>(
            &rx,
            &device,
            &|| Err("no such device".to_string()),
            Duration::from_secs(60),
        );

        assert_eq!(device.get(), Some(Err(STATUS_NO_OUTPUT_DEVICE.to_string())));

        // Which is what the sound indicator then shows.
        let sender = Ok(placeholder_sender());
        let status = status_with_state(Some(&sender), device.get().as_ref(), true);
        assert!(!status.available);
        assert_eq!(status.detail, STATUS_NO_OUTPUT_DEVICE);
    }

    /// Muting is a user-visible feature: the enabled gate must short-circuit
    /// before anything reaches the device. A gate regression would play audio
    /// during a session the user had muted.
    #[test]
    fn play_kind_short_circuits_while_disabled() {
        let was_enabled = is_enabled();
        set_enabled(false);
        let result = play_kind("beep");
        set_enabled(was_enabled);

        assert!(result.is_ok(), "a disabled gate is a no-op, not a failure");
    }

    /// A device that cannot play fails on every flash tick, and a session is
    /// hundreds of ticks: the log must carry the error once, not once per tick.
    /// The latch is a local rather than the process global so this cannot race
    /// the other audio tests, or leave the global latched for them.
    #[test]
    fn play_failure_is_reported_once_per_latch() {
        let latch = AtomicBool::new(false);

        assert!(
            report_play_failure_once(&latch, "beep", "no such device"),
            "the first failure is the one worth reporting",
        );
        assert!(
            latch.load(Ordering::SeqCst),
            "the first report must claim the latch",
        );
        assert!(
            !report_play_failure_once(&latch, "beep", "no such device"),
            "a device that cannot play must not log the same line every tick",
        );

        // Latches are per-scope, so a later session starts reporting again
        // instead of inheriting a stale claim.
        let fresh = AtomicBool::new(false);
        assert!(report_play_failure_once(&fresh, "buzzer", "boom"));
    }

    /// A sender ordinary enough that the branch under test never inspects it.
    fn placeholder_sender() -> Sender<AudioCommand> {
        use std::sync::mpsc;
        mpsc::channel::<AudioCommand>().0
    }

    use rodio::Decoder;
    use std::io::Cursor;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc::TryRecvError;
    use std::sync::{Arc, Mutex};

    #[derive(Debug, PartialEq)]
    enum Event {
        Skip,
        Append(&'static [u8]),
    }

    /// Test sink modeling a queue: appends enqueue, skips dequeue.
    struct RecordingSink {
        events: Mutex<Vec<Event>>,
        queued: AtomicUsize,
    }

    impl RecordingSink {
        fn with_queued(stale: usize) -> Self {
            Self {
                events: Mutex::new(Vec::new()),
                queued: AtomicUsize::new(stale),
            }
        }

        fn events(&self) -> Vec<Event> {
            self.events.lock().expect("event lock").drain(..).collect()
        }
    }

    impl AudioSink for RecordingSink {
        fn queued(&self) -> usize {
            self.queued.load(Ordering::SeqCst)
        }

        fn skip_one(&self) {
            self.events.lock().expect("event lock").push(Event::Skip);
            self.queued
                .try_update(Ordering::SeqCst, Ordering::SeqCst, |n| {
                    Some(n.saturating_sub(1))
                })
                .ok();
        }

        fn append_bytes(&self, data: &'static [u8]) {
            self.events
                .lock()
                .expect("event lock")
                .push(Event::Append(data));
            self.queued.fetch_add(1, Ordering::SeqCst);
        }
    }

    const BEEP: &[u8] = include_bytes!("../../src/assets/beep.wav");

    #[test]
    fn rapid_beeps_cut_stale_audio_before_each_append() {
        // Two stale sounds pending, then two flash ticks.
        let sink = RecordingSink::with_queued(2);
        handle_command(&sink, AudioCommand::Beep);
        handle_command(&sink, AudioCommand::Beep);

        assert_eq!(
            sink.events(),
            vec![
                Event::Skip,
                Event::Skip,
                Event::Append(BEEP),
                Event::Skip,
                Event::Append(BEEP),
            ]
        );
        // Queue never grows: at most the just-appended beep remains.
        assert_eq!(sink.queued(), 1);
    }

    #[test]
    fn feedback_cuts_then_plays_its_payload() {
        let payload: &'static [u8] = b"fake-feedback";
        let sink = RecordingSink::with_queued(3);
        handle_command(&sink, AudioCommand::Feedback(payload));

        assert_eq!(
            sink.events(),
            vec![
                Event::Skip,
                Event::Skip,
                Event::Skip,
                Event::Append(payload),
            ]
        );
        assert_eq!(sink.queued(), 1);
    }

    #[test]
    fn beep_on_empty_queue_appends_without_skips() {
        // Warm path shape: nothing stale, so no skip precedes the append.
        let sink = RecordingSink::with_queued(0);
        handle_command(&sink, AudioCommand::Beep);

        assert_eq!(sink.events(), vec![Event::Append(BEEP)]);
        assert_eq!(sink.queued(), 1);
    }

    #[test]
    fn silence_drains_without_appending() {
        let sink = RecordingSink::with_queued(2);
        handle_command(&sink, AudioCommand::Silence);

        assert_eq!(sink.events(), vec![Event::Skip, Event::Skip]);
        assert_eq!(sink.queued(), 0);
    }

    /// Stopping a session must cut a lingering flash beep, and muting must cut
    /// one mid-session. Both go through `silence`, whose entire job is this one
    /// send, so a test has to be able to see the send happen.
    #[test]
    fn silence_sends_a_cut_to_a_live_worker() {
        let (tx, rx) = channel::<AudioCommand>();
        let sender: Result<Sender<AudioCommand>, String> = Ok(tx);

        silence_with(Some(&sender));

        assert!(
            matches!(rx.try_recv(), Ok(AudioCommand::Silence)),
            "a live worker must receive the cut",
        );
    }

    /// Stopping a session that never made a sound must not open a device, and
    /// neither must one whose worker thread never spawned. The guard is what
    /// keeps a silent session from paying for a cold device open.
    #[test]
    fn silence_skips_when_there_is_nothing_to_cut() {
        // `_tx` is kept alive on purpose: dropping it would disconnect the
        // channel, and a disconnected receiver is a different answer from an
        // empty one.
        let (_tx, rx) = channel::<AudioCommand>();
        let no_worker: Result<Sender<AudioCommand>, String> = Err(STATUS_WORKER_FAILED.to_string());

        silence_with(None);
        silence_with(Some(&no_worker));

        assert!(
            matches!(rx.try_recv(), Err(TryRecvError::Empty)),
            "nothing may be sent when there is no live worker to cut",
        );
    }

    /// Warmup is about the device, never the queue: it may not cut what is
    /// playing and may not append anything, because it runs at session start
    /// where silence is the whole contract. Against the real global sender this
    /// must also return cleanly with or without audio hardware present.
    #[test]
    fn warmup_is_silent_and_never_panics() {
        assert!(matches!(warmup_command(), AudioCommand::Warmup));

        let sink = RecordingSink::with_queued(1);
        assert_eq!(
            handle_command(&sink, AudioCommand::Warmup),
            CutOutcome::Drained,
        );
        assert!(
            sink.events().is_empty(),
            "warmup may not touch a queue that is not its business",
        );
        assert_eq!(sink.queued(), 1, "warmup may not cut what is playing");

        warmup();
    }

    /// Poll for the worker's verdict instead of sleeping a guessed amount. The
    /// open happens on another thread, and a regression on that path must fail
    /// an assertion rather than hang the suite.
    fn wait_for(device: &DeviceState, expected: Option<&Result<(), String>>) -> bool {
        for _ in 0..200 {
            if device.get().as_ref() == expected {
                return true;
            }
            thread::sleep(Duration::from_millis(5));
        }
        device.get().as_ref() == expected
    }

    /// rodio stops playback the moment its sink handle drops, so the handle has
    /// to outlive the open that produced it. A worker that returned only the
    /// `Player` — as this one did — opened the device, published `ready`, and
    /// then played into a stream that was already gone: every status read said
    /// sound was available, and nothing was ever heard.
    #[test]
    fn the_sink_handle_outlives_the_open_that_produced_it() {
        /// Goes dead when dropped, so the assertion is about a lifetime
        /// `run_worker` is responsible for rather than about a value it holds.
        struct Lifeline(Arc<AtomicBool>);
        impl Drop for Lifeline {
            fn drop(&mut self) {
                self.0.store(false, Ordering::SeqCst);
            }
        }

        let alive = Arc::new(AtomicBool::new(true));
        let device = Arc::new(DeviceState::new());
        let (tx, rx) = channel::<AudioCommand>();

        let lifeline = Arc::clone(&alive);
        let verdict = Arc::clone(&device);
        let worker = thread::spawn(move || {
            run_worker(
                &rx,
                &verdict,
                &|| Ok((Lifeline(Arc::clone(&lifeline)), Player::new().0)),
                Duration::from_secs(60),
            );
        });

        // A session starting is what asks for a device, so that is what the
        // worker has to be given before its handle can be judged.
        tx.send(AudioCommand::Warmup).expect("channel is open");
        assert!(
            wait_for(&device, Some(&Ok(()))),
            "the worker must publish a verdict before its handle can be judged",
        );
        assert!(
            alive.load(Ordering::SeqCst),
            "the sink handle must still be alive once the worker says the device is ready",
        );

        drop(tx);
        worker.join().expect("the audio worker must not panic");
    }

    /// A sink whose audio callback never runs: `skip_one` only schedules, which
    /// is exactly what rodio's real `Player` does, and nothing ever drains the
    /// queue. Asking for more skips than there are sounds means the caller is
    /// waiting by spinning instead of by waiting — a busy loop that, on a stream
    /// which stopped pulling, never ends, and that burned a core between every
    /// pair of beeps even while sound was working.
    struct StalledSink {
        queued: AtomicUsize,
        skips: AtomicUsize,
        appends: AtomicUsize,
    }

    impl StalledSink {
        fn with_queued(stale: usize) -> Self {
            Self {
                queued: AtomicUsize::new(stale),
                skips: AtomicUsize::new(0),
                appends: AtomicUsize::new(0),
            }
        }
    }

    impl AudioSink for StalledSink {
        fn queued(&self) -> usize {
            self.queued.load(Ordering::SeqCst)
        }

        fn skip_one(&self) {
            assert!(
                self.skips.load(Ordering::SeqCst) < self.queued(),
                "skip_one scheduled a cut with nothing left to cut: the cut is spinning",
            );
            self.skips.fetch_add(1, Ordering::SeqCst);
        }

        fn append_bytes(&self, _data: &'static [u8]) {
            self.appends.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[test]
    fn a_cut_that_never_drains_gives_up_instead_of_spinning() {
        let sink = StalledSink::with_queued(2);

        assert_eq!(
            handle_command(&sink, AudioCommand::Silence),
            CutOutcome::Stalled,
            "a queue that never empties has to be reported, not waited on forever",
        );
        assert_eq!(
            sink.skips.load(Ordering::SeqCst),
            2,
            "exactly one skip per queued sound — a third is the spin this replaces",
        );
    }

    #[test]
    fn a_sound_is_not_queued_onto_a_stream_that_has_stopped() {
        let sink = StalledSink::with_queued(1);

        assert_eq!(
            handle_command(&sink, AudioCommand::Beep),
            CutOutcome::Stalled,
        );
        assert_eq!(
            sink.appends.load(Ordering::SeqCst),
            0,
            "a beep appended to a dead stream is one nobody hears and a queue that grows",
        );
    }

    /// A machine with no output device must not be re-enumerated on every flash
    /// tick: the cooldown is what turns "retry" into "retry shortly", and
    /// forcing past it is what `AudioCommand::Warmup` exists for, since a
    /// session starting is the moment worth paying an enumeration for.
    #[test]
    fn a_failed_open_is_not_retried_inside_its_cooldown() {
        let device = DeviceState::new();
        let attempts = AtomicUsize::new(0);
        let open = || {
            attempts.fetch_add(1, Ordering::SeqCst);
            Err::<((), Player), String>("device not ready".to_string())
        };
        let mut retry_at = None;
        let long = Duration::from_secs(60);

        assert!(matches!(
            ensure_open(&open, &device, &mut retry_at, long, false),
            Opened::Failed,
        ));
        assert!(
            matches!(
                ensure_open(&open, &device, &mut retry_at, long, false),
                Opened::CoolingDown,
            ),
            "a dead device may not be re-enumerated on the very next tick",
        );
        assert_eq!(attempts.load(Ordering::SeqCst), 1);

        assert!(
            matches!(
                ensure_open(&open, &device, &mut retry_at, long, true),
                Opened::Failed,
            ),
            "a forced open ignores the cooldown",
        );
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
    }

    /// The case this recovery design exists for: the audio server is not up
    /// when the first command arrives, and is up a moment later. Sound has to
    /// come back on its own with the status line following, because the only
    /// alternative was a process restart — the worker used to return on the
    /// first failed open, and the sender half of the channel is never replaced.
    #[test]
    fn a_device_that_comes_back_is_played_again() {
        let device = Arc::new(DeviceState::new());
        let attempts = Arc::new(AtomicUsize::new(0));
        let (tx, rx) = channel::<AudioCommand>();

        let tried = Arc::clone(&attempts);
        let open = move || {
            if tried.fetch_add(1, Ordering::SeqCst) == 0 {
                Err::<((), Player), String>("audio server not up".to_string())
            } else {
                Ok(((), Player::new().0))
            }
        };
        let verdict = Arc::clone(&device);
        let worker = thread::spawn(move || {
            run_worker(&rx, &verdict, &open, Duration::from_millis(10));
        });

        tx.send(AudioCommand::Warmup).expect("channel is open");
        assert!(
            wait_for(&device, Some(&Err(STATUS_NO_OUTPUT_DEVICE.to_string()))),
            "a failed open must reach the status line",
        );

        // A session starting is what forces a retry, cooldown or not.
        tx.send(AudioCommand::Warmup).expect("channel is open");
        assert!(
            wait_for(&device, Some(&Ok(()))),
            "a device that appears later must be reopened without a restart",
        );
        assert_eq!(attempts.load(Ordering::SeqCst), 2);

        drop(tx);
        worker.join().expect("the audio worker must not panic");

        // And the indicator follows it back.
        let sender = Ok(placeholder_sender());
        let status = status_with_state(Some(&sender), device.get().as_ref(), true);
        assert!(status.available, "a recovered device is available again");
        assert_eq!(status.detail, STATUS_READY);
    }

    /// Stopping a silent session must never pay for a device open: that is the
    /// one case that has always been free, and it stays free now that the
    /// worker opens on demand rather than once at startup.
    #[test]
    fn a_cut_with_no_sink_never_opens_a_device() {
        let device = Arc::new(DeviceState::new());
        let attempts = Arc::new(AtomicUsize::new(0));
        let (tx, rx) = channel::<AudioCommand>();

        let tried = Arc::clone(&attempts);
        let open = move || {
            tried.fetch_add(1, Ordering::SeqCst);
            Ok::<((), Player), String>(((), Player::new().0))
        };
        let verdict = Arc::clone(&device);
        let worker =
            thread::spawn(move || run_worker(&rx, &verdict, &open, Duration::from_secs(60)));

        tx.send(AudioCommand::Silence).expect("channel is open");
        drop(tx);
        worker.join().expect("the audio worker must not panic");

        assert_eq!(
            attempts.load(Ordering::SeqCst),
            0,
            "a cut with nothing queued may not buy a cold device open",
        );
        assert_eq!(
            device.get(),
            None,
            "no verdict may be published for an open nobody asked for",
        );
    }

    /// The worker recovers by itself, so the failure latch cannot be one-way: a
    /// fault arriving after a working send is a new episode worth a line, not
    /// the same one told twice.
    #[test]
    fn a_recovered_device_can_report_a_later_failure() {
        let latch = AtomicBool::new(false);

        assert!(report_play_failure_once(&latch, "beep", "channel gone"));
        assert!(!report_play_failure_once(&latch, "beep", "channel gone"));

        clear_play_failure_latch(&latch);
        assert!(
            !latch.load(Ordering::SeqCst),
            "a working send must release the latch",
        );
        assert!(
            report_play_failure_once(&latch, "beep", "channel gone again"),
            "a failure after a recovery is a new episode",
        );
    }

    /// End to end over the real globals, which the pure seam above cannot
    /// reach. Availability is deliberately *not* compared across the two
    /// readings: it describes the output device, and since the worker recovers
    /// on its own the verdict may legitimately move between any two reads —
    /// that movement is the feature. What must not move is the enabled flag
    /// leaking into it, and that is pinned exhaustively by
    /// `status_keeps_the_enabled_flag_and_the_device_independent`.
    #[test]
    fn status_reflects_enabled_flag_not_device_state() {
        let before = is_enabled();
        set_enabled(false);
        let off = status();
        set_enabled(true);
        let on = status();
        set_enabled(before);

        assert!(!off.enabled);
        assert!(on.enabled);
        // Both readings must come from the stable token set the UI renders;
        // the device's own words stay in the log.
        for detail in [off.detail, on.detail] {
            assert!(
                [
                    STATUS_NOT_STARTED,
                    STATUS_READY,
                    STATUS_NO_OUTPUT_DEVICE,
                    STATUS_WORKER_FAILED,
                ]
                .contains(&detail.as_str()),
                "status() may only ever report a stable token, got {detail:?}",
            );
        }
    }

    #[test]
    fn command_for_kind_maps_known_kinds() {
        assert!(matches!(command_for_kind("beep"), Ok(AudioCommand::Beep)));
        assert!(matches!(
            command_for_kind("applause"),
            Ok(AudioCommand::Feedback(_))
        ));
        assert!(matches!(
            command_for_kind("buzzer"),
            Ok(AudioCommand::Feedback(_))
        ));
        assert_eq!(command_for_kind("nope").unwrap_err(), "unknown sound kind");
    }

    #[test]
    fn decode_beep_asset() {
        let data: &'static [u8] = include_bytes!("../../src/assets/beep.wav");
        let cur = Cursor::new(data);
        let dec = Decoder::try_from(cur);
        assert!(dec.is_ok(), "beep.wav should decode as audio");
    }

    #[test]
    fn decode_applause_asset() {
        let data: &'static [u8] = include_bytes!("../../src/assets/applause.wav");
        let cur = Cursor::new(data);
        let dec = Decoder::try_from(cur);
        assert!(dec.is_ok(), "applause.wav should decode as audio");
    }

    #[test]
    fn decode_buzzer_asset() {
        let data: &'static [u8] = include_bytes!("../../src/assets/buzzer.wav");
        let cur = Cursor::new(data);
        let dec = Decoder::try_from(cur);
        assert!(dec.is_ok(), "buzzer.wav should decode as audio");
    }

    /// The sound indicator reads `enabled`/`available`/`detail` off
    /// `get_audio_status`; a renamed field would leave the status line blank.
    #[test]
    fn audio_status_serializes_with_the_frontend_keys() {
        let status = AudioStatus {
            enabled: true,
            available: false,
            detail: STATUS_NO_OUTPUT_DEVICE.to_string(),
        };
        assert_eq!(
            serde_json::to_value(&status).expect("serialize status"),
            serde_json::json!({
                "enabled": true,
                "available": false,
                "detail": STATUS_NO_OUTPUT_DEVICE,
            }),
        );
    }

    /// The kind tokens are lower-case and exact; the UI sends these three, so a
    /// case change or a typo must be rejected rather than silently mapped, and
    /// the mapping must never touch the audio device.
    #[test]
    fn command_for_kind_rejects_everything_but_the_lower_case_tokens() {
        for kind in ["", "Beep", "BEEP", "Applause", "buzzer ", "buzz", "noise"] {
            assert_eq!(
                command_for_kind(kind).unwrap_err(),
                "unknown sound kind",
                "kind {kind:?} must be rejected",
            );
        }
    }
}
