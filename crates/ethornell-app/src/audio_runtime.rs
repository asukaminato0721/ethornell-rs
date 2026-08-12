#[derive(Debug, Clone)]
pub(crate) struct AudioAsset {
    pub(crate) archive: String,
    pub(crate) file: String,
    pub(crate) bytes: Vec<u8>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct NativeCdAudioState {
    /// Whether the target-style CD audio device is currently open.
    pub(crate) opened: bool,
    /// Target public mode mapping: 0=not ready, 1=seek, 2=play,
    /// 3=stop, 4=pause, 5=open, 6=record, -1=unknown.
    pub(crate) mode: i32,
    pub(crate) current_track: Option<u8>,
    pub(crate) notify_requested: bool,
    /// Portable CD-DA replacement tracks keyed by the one-based TMSF track
    /// number used by the target MCI backend.
    pub(crate) tracks: std::collections::BTreeMap<u8, AudioAsset>,
}

#[derive(Debug, Clone)]
pub(crate) struct SoundSlot {
    pub(crate) asset: AudioAsset,
    pub(crate) loop_asset: Option<AudioAsset>,
    pub(crate) looped: bool,
    pub(crate) decode_gain: f64,
    pub(crate) playback_rate: f64,
    pub(crate) panning: f64,
    /// Raw target loader/start parameter forwarded to the sound object's
    /// virtual configuration method. It is not a duration.
    pub(crate) native_start_parameter: i32,
    pub(crate) needs_restart: bool,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct NativeAudioClock {
    position_ms: f64,
    playback_rate: f64,
    running: bool,
}

impl Default for NativeAudioClock {
    fn default() -> Self {
        Self {
            position_ms: 0.0,
            playback_rate: 1.0,
            running: false,
        }
    }
}

impl NativeAudioClock {
    pub(crate) fn start(&mut self, playback_rate: f64) {
        self.position_ms = 0.0;
        self.playback_rate = playback_rate.max(0.0);
        self.running = true;
    }

    pub(crate) fn resume(&mut self) {
        self.running = true;
    }

    pub(crate) fn pause(&mut self) {
        self.running = false;
    }

    pub(crate) fn stop(&mut self) {
        *self = Self::default();
    }

    pub(crate) fn advance(&mut self, elapsed_ms: u64) {
        if self.running {
            self.position_ms += elapsed_ms as f64 * self.playback_rate;
        }
    }

    pub(crate) fn position_ms(self) -> i32 {
        self.position_ms.clamp(0.0, i32::MAX as f64) as i32
    }

    pub(crate) fn is_running(self) -> bool {
        self.running
    }
}

#[derive(Debug, Clone)]
pub(crate) enum AudioCommand {
    Play {
        asset: AudioAsset,
        loop_asset: Option<AudioAsset>,
        channel: i32,
        looped: bool,
        volume: f64,
        decode_gain: f64,
        playback_rate: f64,
        panning: f64,
        fade_ms: u64,
        restart: bool,
    },
    Pause {
        channel: i32,
        paused: bool,
        fade_ms: u64,
    },
    Stop {
        channel: i32,
        fade_ms: u64,
    },
    SetVolume {
        channel: i32,
        volume: f64,
        fade_ms: u64,
    },
    SetPanning {
        channel: i32,
        panning: f64,
        fade_ms: u64,
    },
}

#[cfg(test)]
mod tests {
    use super::NativeAudioClock;

    #[test]
    fn native_audio_clock_follows_virtual_frame_time() {
        let mut clock = NativeAudioClock::default();
        clock.start(2.0);
        clock.advance(25);
        assert_eq!(clock.position_ms(), 50);
        clock.pause();
        clock.advance(25);
        assert_eq!(clock.position_ms(), 50);
        clock.resume();
        clock.advance(10);
        assert_eq!(clock.position_ms(), 70);
        clock.stop();
        assert_eq!(clock.position_ms(), 0);
        assert!(!clock.is_running());
    }
}

pub(crate) fn execute_audio_command(
    audio: Option<&mut ethornell_audio::AudioSystem>,
    command: AudioCommand,
    frontend: &'static str,
) {
    let Some(audio) = audio else {
        tracing::warn!(
            frontend,
            ?command,
            "audio command dropped because backend is unavailable"
        );
        return;
    };
    match command {
        AudioCommand::Play {
            asset,
            loop_asset,
            channel,
            looped,
            volume,
            decode_gain,
            playback_rate,
            panning,
            fade_ms,
            restart,
        } => {
            let result = if let Some(loop_asset) = loop_asset.as_ref() {
                audio.play_intro_loop_on_channel(
                    channel,
                    &asset.bytes,
                    &loop_asset.bytes,
                    looped,
                    volume,
                    decode_gain,
                    playback_rate,
                    panning,
                    fade_ms,
                    restart,
                )
            } else if restart {
                audio
                    .play_on_channel(
                        channel,
                        &asset.bytes,
                        looped,
                        volume,
                        decode_gain,
                        playback_rate,
                        panning,
                        fade_ms,
                    )
                    .map(|()| true)
            } else {
                audio.play_on_channel_if_stopped(
                    channel,
                    &asset.bytes,
                    looped,
                    volume,
                    decode_gain,
                    playback_rate,
                    panning,
                    fade_ms,
                )
            };
            match result {
                Ok(started) => tracing::info!(
                    frontend,
                    channel,
                    looped,
                    volume,
                    decode_gain,
                    playback_rate,
                    panning,
                    fade_ms,
                    started,
                    archive = asset.archive,
                    file = asset.file,
                    loop_archive = loop_asset.as_ref().map(|asset| asset.archive.as_str()),
                    loop_file = loop_asset.as_ref().map(|asset| asset.file.as_str()),
                    "script audio playback requested"
                ),
                Err(err) => tracing::warn!(
                    frontend,
                    channel,
                    archive = asset.archive,
                    file = asset.file,
                    %err,
                    "script audio playback failed"
                ),
            }
        }
        AudioCommand::Pause {
            channel,
            paused,
            fade_ms,
        } => {
            audio.pause_channel(channel, paused, fade_ms);
            tracing::debug!(
                frontend,
                channel,
                paused,
                fade_ms,
                "script audio pause changed"
            );
        }
        AudioCommand::Stop { channel, fade_ms } => {
            audio.stop_channel(channel, fade_ms);
            tracing::debug!(frontend, channel, fade_ms, "script audio channel stopped");
        }
        AudioCommand::SetVolume {
            channel,
            volume,
            fade_ms,
        } => {
            audio.set_channel_volume(channel, volume, fade_ms);
            tracing::debug!(
                frontend,
                channel,
                volume,
                fade_ms,
                "script audio volume changed"
            );
        }
        AudioCommand::SetPanning {
            channel,
            panning,
            fade_ms,
        } => {
            audio.set_channel_panning(channel, panning, fade_ms);
            tracing::debug!(
                frontend,
                channel,
                panning,
                fade_ms,
                "script audio panning changed"
            );
        }
    }
}
