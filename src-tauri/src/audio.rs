use log::{error, info, warn};
use rodio::{Decoder, DeviceSinkBuilder, Player};
use std::io::Cursor;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Sender, channel};

/// Cut semantics: every new sound first drops anything still queued or
/// playing, then starts. Rationale: beeps are time-critical flash ticks
/// (100ms exposure vs ~110ms beep asset); queueing would lag behind the
/// visuals and overlap subsequent ticks.
///
/// Deliberately not `Player::clear()`/`stop()` on this path: `clear()`
/// blocks until the current sound ends, and `append()` after `stop()` can
/// block while the queue flushes (rodio 0.22). `skip_one()` never blocks.
#[derive(Debug, Clone, Copy)]
enum AudioCommand {
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
        Player::skip_one(self);
    }

    fn append_bytes(&self, data: &'static [u8]) {
        match Decoder::try_from(Cursor::new(data)) {
            Ok(src) => self.append(src),
            Err(e) => error!("audio decode error: {}", e),
        }
    }
}

fn handle_command(player: &impl AudioSink, command: AudioCommand) {
    match command {
        AudioCommand::Silence => {
            while player.queued() > 0 {
                player.skip_one();
            }
        }
        AudioCommand::Beep => {
            handle_command(player, AudioCommand::Silence);
            player.append_bytes(include_bytes!("../../src/assets/beep.wav"));
        }
        AudioCommand::Feedback(data) => {
            handle_command(player, AudioCommand::Silence);
            player.append_bytes(data);
        }
    }
}

static AUDIO_SENDER: OnceLock<Result<Sender<AudioCommand>, String>> = OnceLock::new();

fn get_audio_sender() -> Result<&'static Sender<AudioCommand>, String> {
    let value = AUDIO_SENDER.get_or_init(|| {
        let (tx, rx) = channel::<AudioCommand>();

        match std::thread::Builder::new()
            .name("audio-worker".into())
            .spawn(move || match DeviceSinkBuilder::open_default_sink() {
                Ok(stream) => {
                    info!("audio worker initialized output sink");
                    let player = Player::connect_new(stream.mixer());
                    while let Ok(command) = rx.recv() {
                        handle_command(&player, command);
                    }
                    info!("audio worker receiver loop ended");
                }
                Err(e) => {
                    error!("audio worker failed to init output sink: {}", e);
                }
            })
            .map_err(|e| e.to_string())
        {
            Ok(_) => Ok(tx),
            Err(e) => Err(e),
        }
    });
    value.as_ref().map_err(|e| e.clone())
}

fn send_command(command: AudioCommand) -> Result<(), String> {
    let sender = match get_audio_sender() {
        Ok(s) => s,
        Err(e) => {
            warn!("audio sender init failed: {}", e);
            return Err(e);
        }
    };

    sender
        .send(command)
        .map_err(|e| format!("audio send error: {}", e))
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

#[tauri::command]
pub fn play_sound_kind(kind: &str) -> Result<(), String> {
    play_kind(kind)
}

/// Play a sound from Rust (same mapping as the Tauri command).
pub fn play_kind(kind: &str) -> Result<(), String> {
    if !is_enabled() {
        info!("sound disabled; skipping play_kind({})", kind);
        return Ok(());
    }

    let res = command_for_kind(kind).and_then(send_command);

    if let Err(ref e) = res {
        error!("failed to play {}: {}", kind, e);
    }

    res
}

/// Drop any queued or playing audio. Best-effort: failures are ignored so
/// session lifecycle never depends on audio hardware.
pub fn silence() {
    let _ = send_command(AudioCommand::Silence);
}

static SOUND_ENABLED: AtomicBool = AtomicBool::new(true);

pub fn set_enabled(v: bool) {
    SOUND_ENABLED.store(v, Ordering::SeqCst);
}

pub fn is_enabled() -> bool {
    SOUND_ENABLED.load(Ordering::SeqCst)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rodio::Decoder;
    use std::io::Cursor;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

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
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| {
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
    fn silence_drains_without_appending() {
        let sink = RecordingSink::with_queued(2);
        handle_command(&sink, AudioCommand::Silence);

        assert_eq!(sink.events(), vec![Event::Skip, Event::Skip]);
        assert_eq!(sink.queued(), 0);
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
}
