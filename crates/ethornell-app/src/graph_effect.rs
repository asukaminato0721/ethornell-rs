use std::collections::{BTreeMap, VecDeque};
use std::sync::Arc;

use ethornell_image::DecodedImage;
use na_mpeg2_decoder::{Decoder, Demuxer, Frame, Packet, StreamType, frame_to_rgba_bt601_limited};

#[derive(Debug)]
struct MovieDecoder {
    packets: Vec<Packet>,
    decoder: Decoder,
    pending_frames: VecDeque<Arc<Frame>>,
    packet_index: usize,
    frame_index: Option<usize>,
    current_frame: Option<Arc<Frame>>,
    frame_interval_ms: i32,
    flushed: bool,
    failed: bool,
}

impl MovieDecoder {
    fn new(bytes: &[u8]) -> Option<Self> {
        let mut demuxer = Demuxer::new_auto();
        let packets = demuxer
            .push(bytes, None)
            .into_iter()
            .filter(|packet| packet.stream_type == StreamType::MpegVideo)
            .collect::<Vec<_>>();
        if packets.is_empty() {
            return None;
        }
        let frame_rate = packets
            .iter()
            .find_map(|packet| sequence_frame_rate(&packet.data))
            .unwrap_or(25.0);
        Some(Self {
            packets,
            decoder: Decoder::new(),
            pending_frames: VecDeque::new(),
            packet_index: 0,
            frame_index: None,
            current_frame: None,
            frame_interval_ms: (1000.0 / frame_rate).round().max(1.0) as i32,
            flushed: false,
            failed: false,
        })
    }

    fn reset(&mut self) {
        self.decoder = Decoder::new();
        self.pending_frames.clear();
        self.packet_index = 0;
        self.frame_index = None;
        self.current_frame = None;
        self.flushed = false;
        self.failed = false;
    }

    fn frame_at(&mut self, position_ms: i32) -> Option<DecodedImage> {
        let target = (position_ms.max(0) / self.frame_interval_ms.max(1)) as usize;
        if self.frame_index.is_some_and(|index| target < index) {
            self.reset();
        }
        while self.frame_index.is_none_or(|index| index < target) {
            if let Some(frame) = self.next_frame() {
                self.frame_index = Some(self.frame_index.map_or(0, |index| index + 1));
                self.current_frame = Some(frame);
            } else {
                break;
            }
        }
        let frame = self.current_frame.as_ref()?;
        let mut rgba = vec![0; frame.width * frame.height * 4];
        frame_to_rgba_bt601_limited(frame, &mut rgba);
        Some(DecodedImage {
            width: frame.width as u32,
            height: frame.height as u32,
            rgba,
        })
    }

    fn next_frame(&mut self) -> Option<Arc<Frame>> {
        loop {
            if let Some(frame) = self.pending_frames.pop_front() {
                return Some(frame);
            }
            if self.failed {
                return None;
            }
            if let Some(packet) = self.packets.get(self.packet_index) {
                self.packet_index += 1;
                match self.decoder.decode_shared(&packet.data, packet.pts_90k) {
                    Ok(frames) => self.pending_frames.extend(frames),
                    Err(_) => self.failed = true,
                }
                continue;
            }
            if !self.flushed {
                self.flushed = true;
                match self.decoder.flush_shared() {
                    Ok(frames) => self.pending_frames.extend(frames),
                    Err(_) => self.failed = true,
                }
                continue;
            }
            return None;
        }
    }
}

#[derive(Debug)]
pub(crate) struct MovieFrameUpdate {
    pub(crate) bitmap: i32,
    pub(crate) image: DecodedImage,
}

#[derive(Debug)]
struct GraphEffectProcess {
    archive: String,
    resource: String,
    parameters: [i32; 4],
    duration_ms: i32,
    active: bool,
    paused: bool,
    looping: bool,
    position_ms: i32,
    volume: i32,
    result: i32,
    movie: Option<MovieDecoder>,
}

#[derive(Debug, Default)]
pub(crate) struct GraphEffectRegistry {
    processes: BTreeMap<i32, GraphEffectProcess>,
}

impl GraphEffectRegistry {
    pub(crate) fn contains(&self, handle: i32) -> bool {
        self.processes.contains_key(&handle)
    }

    pub(crate) fn configure_loader(&mut self, handle: i32, mode: i32, values: [i32; 4]) -> i32 {
        if handle < 0 {
            return 4;
        }
        self.processes.insert(
            handle,
            GraphEffectProcess {
                archive: String::new(),
                resource: if mode == 0 {
                    "DCProcLoadBurikoMV".into()
                } else {
                    "DCProcLoadBMVHeader".into()
                },
                parameters: values,
                duration_ms: 0,
                active: false,
                paused: false,
                looping: false,
                position_ms: 0,
                volume: 128,
                result: -1,
                movie: None,
            },
        );
        0
    }

    pub(crate) fn configure_media(
        &mut self,
        handle: i32,
        archive: String,
        resource: String,
        bytes: &[u8],
    ) -> i32 {
        if handle < 0 || resource.is_empty() || bytes.is_empty() {
            return 4;
        }
        let duration_ms = probe_mpeg_duration_ms(bytes);
        let Some(movie) = MovieDecoder::new(bytes) else {
            return 2;
        };
        self.processes.insert(
            handle,
            GraphEffectProcess {
                archive,
                resource,
                parameters: [0; 4],
                duration_ms,
                active: false,
                paused: false,
                looping: false,
                position_ms: 0,
                volume: 128,
                result: -1,
                movie: Some(movie),
            },
        );
        0
    }

    pub(crate) fn configure_resource(
        &mut self,
        handle: i32,
        resource: String,
        option1: i32,
        option2: i32,
    ) -> i32 {
        if handle < 0 {
            return 4;
        }
        self.processes.insert(
            handle,
            GraphEffectProcess {
                archive: String::new(),
                resource,
                parameters: [option1, option2, 0, 0],
                duration_ms: 0,
                active: false,
                paused: false,
                looping: false,
                position_ms: 0,
                volume: 128,
                result: -1,
                movie: None,
            },
        );
        0
    }

    pub(crate) fn configure_rect(
        &mut self,
        handle: i32,
        descriptor: i32,
        parameters: [i32; 3],
    ) -> i32 {
        if handle < 0 {
            return 4;
        }
        self.processes.insert(
            handle,
            GraphEffectProcess {
                archive: String::new(),
                resource: String::new(),
                parameters: [descriptor, parameters[0], parameters[1], parameters[2]],
                duration_ms: 0,
                active: false,
                paused: false,
                looping: false,
                position_ms: 0,
                volume: 128,
                result: -1,
                movie: None,
            },
        );
        0
    }

    pub(crate) fn invoke(&mut self, handle: i32) -> ethornell_vm::GraphEffectInvocation {
        let Some(process) = self.processes.get_mut(&handle) else {
            return ethornell_vm::GraphEffectInvocation {
                status: 4,
                duration_ms: 0,
            };
        };
        process.active = true;
        process.paused = false;
        process.position_ms = 0;
        process.result = -1;
        if let Some(movie) = process.movie.as_mut() {
            movie.reset();
        }
        ethornell_vm::GraphEffectInvocation {
            status: 0,
            duration_ms: process.duration_ms,
        }
    }

    pub(crate) fn cancel(&mut self, handle: i32) -> i32 {
        let Some(process) = self.processes.get_mut(&handle) else {
            return 4;
        };
        if !process.active {
            return 1;
        }
        process.active = false;
        0
    }

    pub(crate) fn state(&self, handle: i32) -> i32 {
        match self.processes.get(&handle) {
            Some(process) if process.active => 0,
            Some(_) => 1,
            None => 4,
        }
    }

    pub(crate) fn seek(&mut self, handle: i32, position_ms: i32) -> i32 {
        let Some(process) = self.processes.get_mut(&handle) else {
            return 4;
        };
        if process.movie.is_none() {
            return 1;
        }
        let position_ms = position_ms.max(0).min(process.duration_ms.max(0));
        process.position_ms = position_ms;
        if let Some(movie) = process.movie.as_mut() {
            let _ = movie.frame_at(position_ms);
        }
        0
    }

    pub(crate) fn position_ms(&self, handle: i32) -> Result<i32, i32> {
        let Some(process) = self.processes.get(&handle) else {
            return Err(4);
        };
        if process.movie.is_none() {
            return Err(1);
        }
        Ok(process.position_ms)
    }

    pub(crate) fn set_volume(&mut self, handle: i32, volume: i32) -> i32 {
        let Some(process) = self.processes.get_mut(&handle) else {
            return 4;
        };
        if process.movie.is_none() {
            return 1;
        }
        if !(0..=128).contains(&volume) {
            return 3;
        }
        process.volume = volume;
        0
    }

    pub(crate) fn set_movie_options(&mut self, handle: i32, looping: bool, volume: i32) -> i32 {
        if !(0..=128).contains(&volume) {
            return 3;
        }
        let Some(process) = self.processes.get_mut(&handle) else {
            return 4;
        };
        if process.movie.is_none() {
            return 1;
        }
        process.looping = looping;
        process.volume = volume;
        0
    }

    pub(crate) fn set_paused(&mut self, handle: i32, paused: bool) -> i32 {
        if !(0..0x4000).contains(&handle) {
            return 4;
        }
        let Some(process) = self.processes.get_mut(&handle) else {
            return 1;
        };
        if process.movie.is_none() || !process.active {
            return 1;
        }
        process.paused = paused;
        0
    }

    pub(crate) fn close_movie_slot(&mut self, handle: i32) -> i32 {
        if !(0..0x4000).contains(&handle) {
            return 4;
        }
        match self.processes.get(&handle) {
            Some(process) if process.movie.is_some() => {
                self.processes.remove(&handle);
                0
            }
            _ => 1,
        }
    }

    pub(crate) fn is_movie(&self, handle: i32) -> bool {
        self.processes
            .get(&handle)
            .is_some_and(|process| process.movie.is_some())
    }

    pub(crate) fn tick_movies(&mut self, elapsed_ms: i32) -> Vec<MovieFrameUpdate> {
        let mut updates = Vec::new();
        for (&bitmap, process) in &mut self.processes {
            if !process.active || process.paused {
                continue;
            }
            let Some(movie) = process.movie.as_mut() else {
                continue;
            };
            process.position_ms = process
                .position_ms
                .saturating_add(elapsed_ms.max(0))
                .min(process.duration_ms.max(0));
            if let Some(image) = movie.frame_at(process.position_ms) {
                updates.push(MovieFrameUpdate { bitmap, image });
            }
            if process.duration_ms > 0 && process.position_ms >= process.duration_ms {
                if process.looping {
                    process.position_ms = 0;
                    process.result = -1;
                    if let Some(movie) = process.movie.as_mut() {
                        movie.reset();
                    }
                } else {
                    process.active = false;
                    process.result = 0;
                }
            }
        }
        updates
    }

    pub(crate) fn release(&mut self, handle: i32) -> i32 {
        if self.processes.remove(&handle).is_some() {
            0
        } else {
            4
        }
    }

    pub(crate) fn stop_all_movies(&mut self) {
        for process in self.processes.values_mut() {
            if process.movie.is_some() {
                process.active = false;
                process.result = 0;
            }
        }
    }

    pub(crate) fn set_all_movie_volumes(&mut self, volume: i32) -> i32 {
        if !(0..=128).contains(&volume) {
            return 3;
        }
        for process in self.processes.values_mut() {
            if process.movie.is_some() {
                process.volume = volume;
            }
        }
        0
    }

    pub(crate) fn result(&self, handle: i32) -> Option<i32> {
        self.processes.get(&handle).map(|process| process.result)
    }

    pub(crate) fn duration_ms(&self, handle: i32) -> i32 {
        self.processes
            .get(&handle)
            .map(|process| process.duration_ms)
            .unwrap_or_default()
    }

    pub(crate) fn source(&self, handle: i32) -> Option<(&str, &str)> {
        self.processes
            .get(&handle)
            .map(|process| (process.archive.as_str(), process.resource.as_str()))
    }

    #[cfg(test)]
    pub(crate) fn parameters(&self, handle: i32) -> Option<[i32; 4]> {
        self.processes
            .get(&handle)
            .map(|process| process.parameters)
    }
}

fn probe_mpeg_duration_ms(bytes: &[u8]) -> i32 {
    let mut demuxer = Demuxer::new_auto();
    let packets = demuxer.push(bytes, None);
    let mut first_pts = None;
    let mut last_pts = None;
    let mut pictures = 0_u64;
    let mut frame_rate = None;

    for packet in packets
        .iter()
        .filter(|packet| packet.stream_type == StreamType::MpegVideo)
    {
        if let Some(pts) = packet.pts_90k {
            first_pts.get_or_insert(pts);
            last_pts = Some(pts);
        }
        pictures += count_start_codes(&packet.data, 0x00) as u64;
        frame_rate = frame_rate.or_else(|| sequence_frame_rate(&packet.data));
    }

    let frame_ms = frame_rate
        .map(|fps| (1000.0 / fps).round() as i64)
        .unwrap_or(40);
    let duration = match (first_pts, last_pts) {
        (Some(first), Some(last)) if last > first => {
            ((last - first) * 1000 / 90_000).saturating_add(frame_ms)
        }
        _ if pictures > 0 => {
            ((pictures as f64 * 1000.0) / frame_rate.unwrap_or(25.0)).round() as i64
        }
        _ => 0,
    };
    duration.clamp(0, i32::MAX as i64) as i32
}

fn count_start_codes(bytes: &[u8], code: u8) -> usize {
    bytes
        .windows(4)
        .filter(|window| *window == [0, 0, 1, code])
        .count()
}

fn sequence_frame_rate(bytes: &[u8]) -> Option<f64> {
    const RATES: [f64; 16] = [
        0.0,
        24000.0 / 1001.0,
        24.0,
        25.0,
        30000.0 / 1001.0,
        30.0,
        50.0,
        60000.0 / 1001.0,
        60.0,
        0.0,
        0.0,
        0.0,
        0.0,
        0.0,
        0.0,
        0.0,
    ];
    bytes.windows(8).find_map(|window| {
        (window[..4] == [0, 0, 1, 0xB3])
            .then(|| RATES[(window[7] & 0x0F) as usize])
            .filter(|rate| *rate > 0.0)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sequence_header_frame_rate_is_decoded() {
        let bytes = [0, 0, 1, 0xB3, 0, 0, 0, 3];
        assert_eq!(sequence_frame_rate(&bytes), Some(25.0));
    }

    #[test]
    fn cancelling_one_effect_does_not_stop_its_peer() {
        let mut registry = GraphEffectRegistry::default();
        assert_eq!(registry.configure_resource(3, "crossfade".into(), 1, 2), 0);
        assert_eq!(registry.configure_resource(4, "mask".into(), 3, 4), 0);
        assert_eq!(registry.invoke(3).status, 0);
        assert_eq!(registry.invoke(4).status, 0);

        assert_eq!(registry.cancel(3), 0);
        assert_eq!(registry.state(3), 1);
        assert_eq!(registry.state(4), 0);
    }

    #[test]
    fn rectangular_effect_preserves_native_arguments() {
        let mut registry = GraphEffectRegistry::default();
        assert_eq!(registry.configure_rect(7, 0x1234, [10, 20, 30]), 0);
        assert_eq!(registry.parameters(7), Some([0x1234, 10, 20, 30]));
    }

    #[test]
    fn movie_volume_uses_native_range_and_state_errors() {
        let mut registry = GraphEffectRegistry::default();
        assert_eq!(registry.set_volume(1, 64), 4);
        registry.configure_resource(1, "not-a-movie".into(), 0, 0);
        assert_eq!(registry.set_volume(1, 64), 1);
        assert_eq!(registry.set_volume(1, 129), 1);
    }
}
