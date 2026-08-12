use ethornell_core::{EthornellError, Result};
use kira::dsp::Frame;
use kira::manager::{AudioManager, AudioManagerSettings};
use kira::sound::{
    static_sound::{StaticSoundData, StaticSoundHandle, StaticSoundSettings},
    PlaybackPosition, PlaybackState,
};
use kira::tween::Tween;
use serde::Serialize;
use std::collections::BTreeMap;
use std::io::Cursor;
use std::time::Duration;

#[derive(Debug, Clone, Serialize)]
pub struct AudioInfo {
    pub kind: AudioKind,
    pub header_len: Option<u32>,
    pub file_size: Option<u32>,
    pub sample_len: Option<u32>,
    pub frequency: Option<u32>,
    pub channels: Option<u32>,
    pub payload_offset: Option<usize>,
    pub payload_kind: Option<AudioKind>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum AudioKind {
    Ogg,
    RiffWave,
    BurikoWaveBoxOgg,
    BurikoWaveBoxUnknown,
    Unknown,
}

pub fn probe_audio(data: &[u8]) -> AudioInfo {
    if data.starts_with(b"OggS") {
        return simple_info(AudioKind::Ogg);
    }
    if data.starts_with(b"RIFF") {
        return simple_info(AudioKind::RiffWave);
    }
    if data.len() >= 0x40 && &data[4..8] == b"bw  " {
        let header_len = u32::from_le_bytes(data[0..4].try_into().unwrap());
        let payload_offset = header_len as usize;
        let payload_kind = data
            .get(payload_offset..payload_offset + 4)
            .map(|magic| {
                if magic == b"OggS" {
                    AudioKind::Ogg
                } else if magic == b"RIFF" {
                    AudioKind::RiffWave
                } else {
                    AudioKind::Unknown
                }
            })
            .unwrap_or(AudioKind::Unknown);
        return AudioInfo {
            kind: if payload_kind == AudioKind::Ogg {
                AudioKind::BurikoWaveBoxOgg
            } else {
                AudioKind::BurikoWaveBoxUnknown
            },
            header_len: Some(header_len),
            file_size: read_u32(data, 8),
            sample_len: read_u32(data, 12),
            frequency: read_u32(data, 16),
            channels: read_u32(data, 20),
            payload_offset: Some(payload_offset),
            payload_kind: Some(payload_kind),
        };
    }
    AudioInfo {
        kind: AudioKind::Unknown,
        header_len: None,
        file_size: None,
        sample_len: None,
        frequency: None,
        channels: None,
        payload_offset: None,
        payload_kind: None,
    }
}

pub fn unwrap_buriko_wave_ogg(data: &[u8]) -> Option<&[u8]> {
    let info = probe_audio(data);
    if info.kind != AudioKind::BurikoWaveBoxOgg {
        return None;
    }
    let offset = info.payload_offset?;
    data.get(offset..)
}

fn simple_info(kind: AudioKind) -> AudioInfo {
    AudioInfo {
        kind,
        header_len: None,
        file_size: None,
        sample_len: None,
        frequency: None,
        channels: None,
        payload_offset: Some(0),
        payload_kind: Some(kind),
    }
}

fn read_u32(data: &[u8], offset: usize) -> Option<u32> {
    let bytes: [u8; 4] = data.get(offset..offset + 4)?.try_into().ok()?;
    Some(u32::from_le_bytes(bytes))
}

pub struct AudioSystem {
    manager: AudioManager,
    volume: f64,
    channels: BTreeMap<i32, StaticSoundHandle>,
    next_unmanaged_channel: i32,
}

impl AudioSystem {
    pub fn new() -> Result<Self> {
        let manager = AudioManager::new(AudioManagerSettings::default())
            .map_err(|err| EthornellError::Other(format!("audio init failed: {err}")))?;
        Ok(Self {
            manager,
            volume: 1.0,
            channels: BTreeMap::new(),
            next_unmanaged_channel: i32::MIN,
        })
    }

    pub fn play_ogg_file(&mut self, path: impl AsRef<std::path::Path>) -> Result<()> {
        let data = StaticSoundData::from_file(path, StaticSoundSettings::default())
            .map_err(|err| EthornellError::Other(format!("load audio failed: {err}")))?;
        self.manager
            .play(data)
            .map_err(|err| EthornellError::Other(format!("play audio failed: {err}")))?;
        Ok(())
    }

    pub fn play_from_bytes(&mut self, bytes: &[u8]) -> Result<()> {
        let channel = self.next_unmanaged_channel;
        self.next_unmanaged_channel = self.next_unmanaged_channel.saturating_add(1);
        self.play_on_channel(channel, bytes, false, 1.0, 1.0, 1.0, 0.5, 0)
    }

    pub fn play_on_channel(
        &mut self,
        channel: i32,
        bytes: &[u8],
        looped: bool,
        volume: f64,
        decode_gain: f64,
        playback_rate: f64,
        panning: f64,
        fade_in_ms: u64,
    ) -> Result<()> {
        self.play_on_channel_inner(
            channel,
            bytes,
            looped,
            volume,
            decode_gain,
            playback_rate,
            panning,
            fade_in_ms,
            true,
        )
        .map(|_| ())
    }

    pub fn play_on_channel_if_stopped(
        &mut self,
        channel: i32,
        bytes: &[u8],
        looped: bool,
        volume: f64,
        decode_gain: f64,
        playback_rate: f64,
        panning: f64,
        fade_in_ms: u64,
    ) -> Result<bool> {
        self.play_on_channel_inner(
            channel,
            bytes,
            looped,
            volume,
            decode_gain,
            playback_rate,
            panning,
            fade_in_ms,
            false,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn play_intro_loop_on_channel(
        &mut self,
        channel: i32,
        intro_bytes: &[u8],
        loop_bytes: &[u8],
        looped: bool,
        volume: f64,
        decode_gain: f64,
        playback_rate: f64,
        panning: f64,
        fade_in_ms: u64,
        restart: bool,
    ) -> Result<bool> {
        if !restart && self.channel_is_active(channel) {
            return Ok(false);
        }

        let intro = decode_static_sound(intro_bytes)?;
        let loop_sound = decode_static_sound(loop_bytes)?;
        if intro.sample_rate != loop_sound.sample_rate {
            return Err(EthornellError::Other(format!(
                "BGM pair sample-rate mismatch: intro={}Hz loop={}Hz",
                intro.sample_rate, loop_sound.sample_rate
            )));
        }

        let loop_start = intro.frames.len();
        let mut frames = Vec::with_capacity(loop_start.saturating_add(loop_sound.frames.len()));
        frames.extend_from_slice(&intro.frames);
        frames.extend_from_slice(&loop_sound.frames);

        let mut settings = playback_settings(volume, playback_rate, panning, fade_in_ms);
        if looped {
            settings = settings
                .loop_region(PlaybackPosition::Samples(loop_start.min(i64::MAX as usize) as i64)..);
        }
        let mut data = StaticSoundData {
            sample_rate: intro.sample_rate,
            frames: frames.into(),
            settings,
        };
        apply_decode_gain(&mut data, decode_gain);
        self.start_static_sound(channel, data)
    }

    fn play_on_channel_inner(
        &mut self,
        channel: i32,
        bytes: &[u8],
        looped: bool,
        volume: f64,
        decode_gain: f64,
        playback_rate: f64,
        panning: f64,
        fade_in_ms: u64,
        restart: bool,
    ) -> Result<bool> {
        if !restart && self.channel_is_active(channel) {
            return Ok(false);
        }
        let payload = unwrap_buriko_wave_ogg(bytes).unwrap_or(bytes);
        let mut settings = playback_settings(volume, playback_rate, panning, fade_in_ms);
        if looped {
            settings = settings.loop_region(..);
        }
        let mut data = StaticSoundData::from_cursor(Cursor::new(payload.to_vec()), settings)
            .map_err(|err| EthornellError::Other(format!("load audio bytes failed: {err}")))?;
        apply_decode_gain(&mut data, decode_gain);
        self.start_static_sound(channel, data)
    }

    fn start_static_sound(&mut self, channel: i32, data: StaticSoundData) -> Result<bool> {
        let handle = self
            .manager
            .play(data)
            .map_err(|err| EthornellError::Other(format!("play audio bytes failed: {err}")))?;
        if let Some(mut previous) = self.channels.insert(channel, handle) {
            let _ = previous.stop(Tween::default());
        }
        Ok(true)
    }

    pub fn stop_all(&mut self) {
        for handle in self.channels.values_mut() {
            let _ = handle.stop(Tween::default());
        }
        self.channels.clear();
    }

    pub fn stop_channel(&mut self, channel: i32, fade_out_ms: u64) {
        let Some(handle) = self.channels.get_mut(&channel) else {
            return;
        };
        let _ = handle.stop(Tween {
            duration: Duration::from_millis(fade_out_ms),
            ..Tween::default()
        });
    }

    pub fn pause_channel(&mut self, channel: i32, paused: bool, fade_ms: u64) {
        let Some(handle) = self.channels.get_mut(&channel) else {
            return;
        };
        let tween = Tween {
            duration: Duration::from_millis(fade_ms),
            ..Tween::default()
        };
        if paused {
            let _ = handle.pause(tween);
        } else {
            let _ = handle.resume(tween);
        }
    }

    pub fn set_channel_volume(&mut self, channel: i32, volume: f64, fade_ms: u64) {
        let Some(handle) = self.channels.get_mut(&channel) else {
            return;
        };
        let _ = handle.set_volume(
            volume.clamp(0.0, 1.0),
            Tween {
                duration: Duration::from_millis(fade_ms),
                ..Tween::default()
            },
        );
    }

    pub fn set_channel_panning(&mut self, channel: i32, panning: f64, fade_ms: u64) {
        let Some(handle) = self.channels.get_mut(&channel) else {
            return;
        };
        let _ = handle.set_panning(
            panning.clamp(0.0, 1.0),
            Tween {
                duration: Duration::from_millis(fade_ms),
                ..Tween::default()
            },
        );
    }

    pub fn channel_is_active(&self, channel: i32) -> bool {
        self.channels
            .get(&channel)
            .is_some_and(|handle| handle.state() != PlaybackState::Stopped)
    }

    pub fn set_volume(&mut self, volume: f64) {
        self.volume = volume.clamp(0.0, 1.0);
    }

    pub fn volume(&self) -> f64 {
        self.volume
    }
}

fn decode_static_sound(bytes: &[u8]) -> Result<StaticSoundData> {
    let payload = unwrap_buriko_wave_ogg(bytes).unwrap_or(bytes);
    StaticSoundData::from_cursor(
        Cursor::new(payload.to_vec()),
        StaticSoundSettings::default(),
    )
    .map_err(|err| EthornellError::Other(format!("load audio bytes failed: {err}")))
}

fn playback_settings(
    volume: f64,
    playback_rate: f64,
    panning: f64,
    fade_in_ms: u64,
) -> StaticSoundSettings {
    let mut settings = StaticSoundSettings::default()
        .volume(volume.clamp(0.0, 1.0))
        .playback_rate(playback_rate.clamp(0.01, 16.0))
        .panning(panning.clamp(0.0, 1.0));
    if fade_in_ms > 0 {
        settings = settings.fade_in_tween(Tween {
            duration: Duration::from_millis(fade_in_ms),
            ..Tween::default()
        });
    }
    settings
}

fn apply_decode_gain(data: &mut StaticSoundData, decode_gain: f64) {
    let decode_gain = decode_gain.max(0.0) as f32;
    if (decode_gain - 1.0).abs() <= f32::EPSILON {
        return;
    }
    data.frames = data
        .frames
        .iter()
        .map(|frame| {
            Frame::new(
                (frame.left * decode_gain).clamp(-1.0, 1.0),
                (frame.right * decode_gain).clamp(-1.0, 1.0),
            )
        })
        .collect::<Vec<_>>()
        .into();
}
