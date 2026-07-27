use std::collections::BTreeMap;

use na_mpeg2_decoder::{Demuxer, StreamType};

#[derive(Debug)]
struct GraphEffectProcess {
    archive: String,
    resource: String,
    duration_ms: i32,
    active: bool,
    position_ms: i32,
    result: i32,
}

#[derive(Debug, Default)]
pub(crate) struct GraphEffectRegistry {
    processes: BTreeMap<i32, GraphEffectProcess>,
}

impl GraphEffectRegistry {
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
        self.processes.insert(
            handle,
            GraphEffectProcess {
                archive,
                resource,
                duration_ms,
                active: false,
                position_ms: 0,
                result: -1,
            },
        );
        0
    }

    pub(crate) fn configure_placeholder(&mut self, handle: i32) -> i32 {
        if handle < 0 {
            return 4;
        }
        self.processes
            .entry(handle)
            .or_insert_with(|| GraphEffectProcess {
                archive: String::new(),
                resource: String::new(),
                duration_ms: 0,
                active: false,
                position_ms: 0,
                result: -1,
            });
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
        process.position_ms = 0;
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

    pub(crate) fn cancel_all(&mut self) -> i32 {
        let mut found = false;
        for process in self.processes.values_mut() {
            found |= process.active;
            process.active = false;
        }
        if found {
            0
        } else {
            1
        }
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
        if !process.active {
            return 1;
        }
        process.position_ms = position_ms.max(0).min(process.duration_ms.max(0));
        0
    }

    pub(crate) fn release(&mut self, handle: i32) -> i32 {
        if self.processes.remove(&handle).is_some() {
            0
        } else {
            4
        }
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
}
