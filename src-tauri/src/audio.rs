use log::{error, info, warn};
use once_cell::sync::OnceCell;
use rodio::{Decoder, DeviceSinkBuilder, Player};
use std::io::Cursor;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Sender, channel};

static AUDIO_SENDER: OnceCell<Sender<&'static [u8]>> = OnceCell::new();

fn get_audio_sender() -> Result<&'static Sender<&'static [u8]>, String> {
    AUDIO_SENDER.get_or_try_init(|| {
        let (tx, rx) = channel::<&'static [u8]>();

        std::thread::Builder::new()
            .name("audio-worker".into())
            .spawn(move || match DeviceSinkBuilder::open_default_sink() {
                Ok(stream) => {
                    info!("audio worker initialized output sink");
                    let player = Player::connect_new(stream.mixer());
                    while let Ok(data) = rx.recv() {
                        let cursor = Cursor::new(data);
                        match Decoder::try_from(cursor) {
                            Ok(src) => {
                                player.append(src);
                            }
                            Err(e) => error!("audio decode error: {}", e),
                        }
                    }
                    info!("audio worker receiver loop ended");
                }
                Err(e) => {
                    error!("audio worker failed to init output sink: {}", e);
                }
            })
            .map_err(|e| e.to_string())?;

        Ok(tx)
    })
}

fn play_bytes(data: &'static [u8]) -> Result<(), String> {
    let sender = match get_audio_sender() {
        Ok(s) => s,
        Err(e) => {
            warn!("audio sender init failed: {}", e);
            return Err(e);
        }
    };

    sender
        .send(data)
        .map_err(|e| format!("audio send error: {}", e))
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

    let res = match kind {
        "beep" => play_bytes(include_bytes!("../../src/assets/beep.wav")),
        "applause" => play_bytes(include_bytes!("../../src/assets/applause.wav")),
        "buzzer" => play_bytes(include_bytes!("../../src/assets/buzzer.wav")),
        _ => Err("unknown sound kind".to_string()),
    };

    if let Err(ref e) = res {
        error!("failed to play {}: {}", kind, e);
    }

    res
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
    use rodio::Decoder;
    use std::io::Cursor;

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
