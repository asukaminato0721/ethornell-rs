use ethornell_core::{EthornellError, Result};
use kira::manager::{AudioManager, AudioManagerSettings};
use kira::sound::static_sound::{StaticSoundData, StaticSoundSettings};
use serde::Serialize;
use std::io::Cursor;

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
}

impl AudioSystem {
    pub fn new() -> Result<Self> {
        let manager = AudioManager::new(AudioManagerSettings::default())
            .map_err(|err| EthornellError::Other(format!("audio init failed: {err}")))?;
        Ok(Self {
            manager,
            volume: 1.0,
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
        let payload = unwrap_buriko_wave_ogg(bytes).unwrap_or(bytes);
        let data = StaticSoundData::from_cursor(
            Cursor::new(payload.to_vec()),
            StaticSoundSettings::default(),
        )
        .map_err(|err| EthornellError::Other(format!("load audio bytes failed: {err}")))?;
        self.manager
            .play(data)
            .map_err(|err| EthornellError::Other(format!("play audio bytes failed: {err}")))?;
        Ok(())
    }

    pub fn stop_all(&mut self) {
        // Kira stops sounds through handles/tracks. The first skeleton does not
        // retain handles yet, so this becomes real once playback routing exists.
    }

    pub fn set_volume(&mut self, volume: f64) {
        self.volume = volume.clamp(0.0, 1.0);
    }

    pub fn volume(&self) -> f64 {
        self.volume
    }
}
