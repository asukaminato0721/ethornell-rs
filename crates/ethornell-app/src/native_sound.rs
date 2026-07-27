use super::*;

#[cfg(test)]
const OWNED_A0_IDS: &[u16] = &[
    0x00, 0x08, 0x09, 0x10, 0x11, 0x12, 0x14, 0x15, 0x16, 0x17, 0x18, 0x19, 0x1C, 0x20, 0x21,
    0x22, 0x23, 0x24, 0x25, 0x26, 0x27, 0x28, 0x2C, 0x2F, 0x80, 0x81, 0x84, 0x85, 0x86, 0xC0,
];

impl RuntimeTraceApi {
    pub(super) fn dispatch_native_sound(
        &mut self,
        group: u8,
        id: u16,
        stack: &mut Vec<ethornell_vm::Value>,
    ) -> Option<ethornell_vm::VmResult<ethornell_vm::Value>> {
        if group != 0xA0 {
            return None;
        }

        let value = match id {
            // sub_487010 pushes the engine's fixed software channel count.
            0x00 => ethornell_vm::Value::Int(20),
            // sub_493B60/sub_493B90 cache the two native mixer banks.
            0x08 => {
                let volume = pop_int_value(stack).unwrap_or(128);
                let mixer_group = pop_int_value(stack).unwrap_or_default();
                if let Some(slot) = usize::try_from(mixer_group)
                    .ok()
                    .filter(|slot| *slot < self.bgm_channel_volumes.len())
                {
                    self.bgm_channel_volumes[slot] = volume.clamp(0, 128) as u8;
                }
                ethornell_vm::Value::None
            }
            0x09 => {
                let volume = pop_int_value(stack).unwrap_or(128);
                let channel = pop_int_value(stack).unwrap_or_default();
                self.set_native_sound_channel_volume(channel, volume, 0);
                ethornell_vm::Value::None
            }
            // sub_4870B0 -> sub_493C00 loads one decoded sound channel.
            0x10 => {
                let volume = pop_int_value(stack).unwrap_or(128);
                let file = pop_string_value(stack).unwrap_or_default();
                let channel = pop_int_value(stack).unwrap_or_default();
                let loaded = self.load_native_bgm_slot(channel, "", &file, false);
                if let Some(slot) = usize::try_from(channel)
                    .ok()
                    .filter(|slot| *slot < self.bgm_channel_volumes.len())
                {
                    self.bgm_channel_volumes[slot] = volume.clamp(0, 128) as u8;
                }
                tracing::debug!(channel, file, volume, loaded, "SoundLoadNativeChannel");
                ethornell_vm::Value::None
            }
            // sub_487180 -> sub_493DB0 is the archive-aware BGM loader.
            0x11 => {
                let pan = pop_int_value(stack).unwrap_or(64);
                let volume = pop_int_value(stack).unwrap_or(128);
                let file = pop_string_value(stack).unwrap_or_default();
                let archive = pop_string_value(stack).unwrap_or_default();
                let channel = pop_int_value(stack).unwrap_or_default();
                self.play_native_bgm(channel, &archive, &file, volume, pan);
                ethornell_vm::Value::None
            }
            // sub_487280 -> sub_4940D0 loads the native two-part BGM form.
            0x12 => {
                let pan = pop_int_value(stack).unwrap_or(64);
                let volume = pop_int_value(stack).unwrap_or(128);
                let looped = pop_int_value(stack).unwrap_or_default() != 0;
                let loop_file = pop_string_value(stack).unwrap_or_default();
                let intro_file = pop_string_value(stack).unwrap_or_default();
                let archive = pop_string_value(stack).unwrap_or_default();
                let channel = pop_int_value(stack).unwrap_or_default();
                let file = if looped && !loop_file.is_empty() {
                    loop_file.as_str()
                } else {
                    intro_file.as_str()
                };
                let loaded = self.load_native_bgm_slot(channel, &archive, file, looped);
                if let Some(slot) = usize::try_from(channel)
                    .ok()
                    .filter(|slot| *slot < self.bgm_channel_volumes.len())
                {
                    self.bgm_channel_volumes[slot] = volume.clamp(0, 128) as u8;
                }
                self.audio_requests.push_back(AudioCommand::SetPanning {
                    channel,
                    panning: native_audio_panning(pan),
                    fade_ms: 0,
                });
                tracing::info!(
                    channel,
                    archive,
                    intro_file,
                    loop_file,
                    looped,
                    volume,
                    pan,
                    loaded,
                    "SoundLoadBgmPair"
                );
                ethornell_vm::Value::None
            }
            // sub_4A3150 forwards pause/resume to the resident BGM buffer.
            0x14 => {
                let action = pop_int_value(stack).unwrap_or_default();
                let channel = pop_int_value(stack).unwrap_or_default();
                if action != 0 && self.play_loaded_native_bgm(channel) {
                    set_bgm_channel_active(&mut self.bgm_channel_active, channel, true);
                    self.start_bgm_clock(channel, 1.0);
                } else {
                    self.audio_requests.push_back(AudioCommand::Pause {
                        channel,
                        paused: action == 0,
                        fade_ms: 0,
                    });
                    set_bgm_channel_active(
                        &mut self.bgm_channel_active,
                        channel,
                        action != 0,
                    );
                    self.set_bgm_clock_paused(channel, action == 0);
                }
                ethornell_vm::Value::None
            }
            // sub_487400 -> sub_4942B0 returns the BGM playback cursor.
            // Its second argument is an optional output cell for the backend
            // timer; the return value itself is the cursor used by scripts.
            0x15 => {
                let _timer_destination = stack.pop();
                let channel = pop_int_value(stack).unwrap_or_default();
                ethornell_vm::Value::Int(self.bgm_position(channel))
            }
            0x16 => {
                let duration = pop_int_value(stack).unwrap_or_default();
                let volume = pop_int_value(stack).unwrap_or_default();
                let channel = pop_int_value(stack).unwrap_or_default();
                if let Some(slot) = usize::try_from(channel)
                    .ok()
                    .filter(|slot| *slot < self.bgm_channel_volumes.len())
                {
                    self.bgm_channel_volumes[slot] = volume.clamp(0, 128) as u8;
                }
                self.audio_requests.push_back(AudioCommand::SetVolume {
                    channel,
                    volume: native_audio_volume(volume),
                    fade_ms: native_audio_duration(duration),
                });
                ethornell_vm::Value::None
            }
            // sub_4A2C20 maps 0..128 around center 64 to DirectSound pan.
            0x17 => {
                let pan = pop_int_value(stack).unwrap_or(64);
                let channel = pop_int_value(stack).unwrap_or_default();
                self.audio_requests.push_back(AudioCommand::SetPanning {
                    channel,
                    panning: native_audio_panning(pan),
                    fade_ms: 0,
                });
                ethornell_vm::Value::None
            }
            // sub_4A29F0 and sub_4A2990 schedule fades to full/silent.
            0x18 => {
                let duration = pop_int_value(stack).unwrap_or_default();
                let channel = pop_int_value(stack).unwrap_or_default();
                self.audio_requests.push_back(AudioCommand::SetVolume {
                    channel,
                    volume: 1.0,
                    fade_ms: native_audio_duration(duration),
                });
                ethornell_vm::Value::None
            }
            0x19 => {
                let duration = pop_int_value(stack).unwrap_or_default();
                let channel = pop_int_value(stack).unwrap_or_default();
                self.audio_requests.push_back(AudioCommand::SetVolume {
                    channel,
                    volume: 0.0,
                    fade_ms: native_audio_duration(duration),
                });
                ethornell_vm::Value::None
            }
            // sub_4A2BB0 changes one of the sixteen group volumes.
            0x1C => {
                let volume = pop_int_value(stack).unwrap_or(128);
                let group = pop_int_value(stack).unwrap_or_default();
                if let Some(slot) = usize::try_from(group)
                    .ok()
                    .filter(|slot| *slot < self.sound_group_volumes.len())
                {
                    self.sound_group_volumes[slot] = volume.clamp(0, 128) as u8;
                }
                ethornell_vm::Value::None
            }
            // sub_4875A0/sub_487650/sub_487770/sub_487960 all construct
            // CProcLoadSound.  The fixed-point fields are decoder gain and
            // playback-rate scaling; they are not loop flags.
            0x20 => {
                let file = pop_string_value(stack).unwrap_or_default();
                let archive = pop_string_value(stack).unwrap_or_default();
                let channel = pop_int_value(stack).unwrap_or_default();
                let loaded =
                    self.load_native_sound_slot_from_archive(channel, &archive, &file, 1.0, 1.0, 0);
                self.sound_load_process_handle(channel, loaded)
            }
            0x21 => {
                let decode_gain = native_audio_fixed(pop_int_value(stack).unwrap_or(65_536));
                let fade_in_ms = native_audio_duration(pop_int_value(stack).unwrap_or_default());
                let file = pop_string_value(stack).unwrap_or_default();
                let archive = pop_string_value(stack).unwrap_or_default();
                let channel = pop_int_value(stack).unwrap_or_default();
                let loaded = self.load_native_sound_slot_from_archive(
                    channel,
                    &archive,
                    &file,
                    decode_gain,
                    1.0,
                    fade_in_ms,
                );
                tracing::info!(
                    channel,
                    archive,
                    file,
                    fade_in_ms,
                    decode_gain,
                    loaded,
                    "SoundLoadSlotEx"
                );
                self.sound_load_process_handle(channel, loaded)
            }
            // sub_487740 releases a resident static sound buffer.
            0x22 => {
                let channel = pop_int_value(stack).unwrap_or_default();
                self.release_native_sound_playback(channel);
                self.sound_slots.remove(&channel);
                ethornell_vm::Value::None
            }
            0x23 => {
                let decode_gain = native_audio_fixed(pop_int_value(stack).unwrap_or(65_536));
                let fade_in_ms = native_audio_duration(pop_int_value(stack).unwrap_or_default());
                let file = pop_string_value(stack).unwrap_or_default();
                let archive = pop_string_value(stack).unwrap_or_default();
                let channel = pop_int_value(stack).unwrap_or_default();
                let loaded = self.load_native_sound_slot_from_archive(
                    channel,
                    &archive,
                    &file,
                    decode_gain,
                    2.0,
                    fade_in_ms,
                );
                self.sound_load_process_handle(channel, loaded)
            }
            // sub_487860 starts the resident buffer and returns its position.
            0x24 => {
                let pan = pop_int_value(stack).unwrap_or(64);
                let volume = pop_int_value(stack).unwrap_or(128);
                let channel = pop_int_value(stack).unwrap_or_default();
                let sound = self.sound_slots.get_mut(&channel).map(|sound| {
                    let restart = std::mem::take(&mut sound.needs_restart);
                    (sound.clone(), restart)
                });
                if let Some((sound, restart)) = sound {
                    let was_active =
                        sound_channel_active(&self.sound_channel_active, channel);
                    self.audio_requests.push_back(AudioCommand::Play {
                        asset: sound.asset,
                        channel: native_se_audio_channel(channel),
                        looped: sound.looped,
                        volume: native_audio_volume(volume),
                        decode_gain: sound.decode_gain,
                        playback_rate: sound.playback_rate,
                        panning: native_audio_panning(pan),
                        fade_ms: sound.fade_in_ms,
                        restart,
                    });
                    set_sound_channel_active(&mut self.sound_channel_active, channel, true);
                    if restart || !was_active {
                        self.start_sound_clock(channel, sound.playback_rate);
                    }
                }
                ethornell_vm::Value::Int(self.sound_position(channel))
            }
            // sub_4878F0 stops without freeing the resident buffer.
            0x25 => {
                let channel = pop_int_value(stack).unwrap_or_default();
                self.audio_requests.push_back(AudioCommand::Stop {
                    channel: native_se_audio_channel(channel),
                    fade_ms: 0,
                });
                set_sound_channel_active(&mut self.sound_channel_active, channel, false);
                self.stop_sound_clock(channel);
                ethornell_vm::Value::None
            }
            // sub_4A2A50 schedules a per-channel fade to silence.
            0x26 => {
                let duration = pop_int_value(stack).unwrap_or_default();
                let channel = pop_int_value(stack).unwrap_or_default();
                self.audio_requests.push_back(AudioCommand::SetVolume {
                    channel: native_se_audio_channel(channel),
                    volume: 0.0,
                    fade_ms: native_audio_duration(duration),
                });
                ethornell_vm::Value::None
            }
            0x27 => {
                let playback_rate = native_audio_fixed(pop_int_value(stack).unwrap_or(65_536));
                let decode_gain = native_audio_fixed(pop_int_value(stack).unwrap_or(65_536));
                let fade_in_ms = native_audio_duration(pop_int_value(stack).unwrap_or_default());
                let file = pop_string_value(stack).unwrap_or_default();
                let archive = pop_string_value(stack).unwrap_or_default();
                let channel = pop_int_value(stack).unwrap_or_default();
                let loaded = self.load_native_sound_slot_from_archive(
                    channel,
                    &archive,
                    &file,
                    decode_gain,
                    playback_rate,
                    fade_in_ms,
                );
                self.sound_load_process_handle(channel, loaded)
            }
            // sub_452530 constructs the dynamic registration process. Its
            // sound descriptor is VM-owned; retain the process identity.
            0x28 => {
                let _args = pop_args(stack, 5);
                let handle = self.alloc_object();
                self.graph_process_handles.insert(handle);
                ethornell_vm::Value::Int(handle)
            }
            // sub_4A2890 sets one of the 64 per-channel volumes.
            0x2C => {
                let volume = pop_int_value(stack).unwrap_or(128);
                let channel = pop_int_value(stack).unwrap_or_default();
                self.set_native_sound_channel_volume(channel, volume, 0);
                ethornell_vm::Value::None
            }
            // sub_487BA0 -> sub_4943E0 returns the current playback position
            // in milliseconds, including the native fixed playback scale.
            0x2F => {
                let channel = pop_int_value(stack).unwrap_or_default();
                ethornell_vm::Value::Int(self.sound_position(channel))
            }
            // MCI CD-audio is unavailable on the portable backend. Native
            // wrappers return zero for these unsuccessful operations.
            0x80 | 0x81 | 0x85 => ethornell_vm::Value::None,
            0x84 => {
                let _args = pop_args(stack, 2);
                ethornell_vm::Value::Int(0)
            }
            // The VM owns the output pointer and writes mode -1.
            0x86 => {
                let _destination = stack.pop();
                ethornell_vm::Value::Int(0)
            }
            // sub_4944D0 is the legacy asynchronous PlaySound path.
            0xC0 => {
                let file = pop_string_value(stack).unwrap_or_default();
                let channel = 63;
                let loaded = self.load_native_sound_slot(channel, &file, 1.0, 1.0, 0);
                if loaded {
                    self.play_native_sound_slot(channel, 128, 64);
                }
                ethornell_vm::Value::Int(i32::from(loaded))
            }
            _ => return None,
        };
        Some(Ok(value))
    }

    fn load_native_sound_slot(
        &mut self,
        channel: i32,
        file: &str,
        decode_gain: f64,
        playback_rate: f64,
        fade_in_ms: u64,
    ) -> bool {
        let Some(entry) = self.manager.find(file) else {
            return false;
        };
        let Ok(bytes) = self.manager.read_by_entry_decoded(&entry) else {
            return false;
        };
        let archive = entry
            .archive_path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
            .to_string();
        self.sound_slots.insert(
            channel,
            SoundSlot {
                asset: AudioAsset {
                    archive,
                    file: file.to_string(),
                    bytes,
                },
                looped: false,
                decode_gain,
                playback_rate,
                fade_in_ms,
                needs_restart: true,
            },
        );
        self.release_native_sound_playback(channel);
        true
    }

    fn load_native_bgm_slot(
        &mut self,
        channel: i32,
        archive: &str,
        file: &str,
        looped: bool,
    ) -> bool {
        let entry = if archive.is_empty() {
            self.manager.find(file)
        } else {
            find_runtime_resource(&self.manager, archive, file)
        };
        let Some(entry) = entry else {
            return false;
        };
        let Ok(bytes) = self.manager.read_by_entry_decoded(&entry) else {
            return false;
        };
        let resolved_archive = entry
            .archive_path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(archive)
            .to_string();
        self.bgm_slots.insert(
            channel,
            SoundSlot {
                asset: AudioAsset {
                    archive: resolved_archive,
                    file: file.to_string(),
                    bytes,
                },
                looped,
                decode_gain: 1.0,
                playback_rate: 1.0,
                fade_in_ms: 0,
                needs_restart: true,
            },
        );
        self.audio_requests.push_back(AudioCommand::Stop {
            channel,
            fade_ms: 0,
        });
        set_bgm_channel_active(&mut self.bgm_channel_active, channel, false);
        true
    }

    fn play_loaded_native_bgm(&mut self, channel: i32) -> bool {
        let Some((sound, restart)) = self.bgm_slots.get_mut(&channel).map(|sound| {
            let restart = std::mem::take(&mut sound.needs_restart);
            (sound.clone(), restart)
        }) else {
            return false;
        };
        if !restart {
            return false;
        }
        let volume = self
            .bgm_channel_volumes
            .get(channel.max(0) as usize)
            .copied()
            .unwrap_or(128);
        self.audio_requests.push_back(AudioCommand::Play {
            asset: sound.asset,
            channel,
            looped: sound.looped,
            volume: native_audio_volume(i32::from(volume)),
            decode_gain: sound.decode_gain,
            playback_rate: sound.playback_rate,
            panning: 0.5,
            fade_ms: sound.fade_in_ms,
            restart,
        });
        true
    }

    fn load_native_sound_slot_from_archive(
        &mut self,
        channel: i32,
        archive: &str,
        file: &str,
        decode_gain: f64,
        playback_rate: f64,
        fade_in_ms: u64,
    ) -> bool {
        let Some(entry) = find_runtime_resource(&self.manager, archive, file) else {
            return false;
        };
        let Ok(bytes) = self.manager.read_by_entry_decoded(&entry) else {
            return false;
        };
        self.sound_slots.insert(
            channel,
            SoundSlot {
                asset: AudioAsset {
                    archive: archive.to_string(),
                    file: file.to_string(),
                    bytes,
                },
                looped: false,
                decode_gain,
                playback_rate,
                fade_in_ms,
                needs_restart: true,
            },
        );
        self.release_native_sound_playback(channel);
        true
    }

    fn release_native_sound_playback(&mut self, channel: i32) {
        self.audio_requests.push_back(AudioCommand::Stop {
            channel: native_se_audio_channel(channel),
            fade_ms: 0,
        });
        set_sound_channel_active(&mut self.sound_channel_active, channel, false);
        self.stop_sound_clock(channel);
    }

    fn sound_load_process_handle(&mut self, channel: i32, loaded: bool) -> ethornell_vm::Value {
        let handle = self.alloc_object();
        self.graph_process_handles.insert(handle);
        tracing::debug!(channel, handle, loaded, "SoundLoadProcess");
        ethornell_vm::Value::Int(handle)
    }

    fn set_native_sound_channel_volume(&mut self, channel: i32, volume: i32, fade_ms: i32) {
        if let Some(slot) = usize::try_from(channel)
            .ok()
            .filter(|slot| *slot < self.sound_channel_volumes.len())
        {
            self.sound_channel_volumes[slot] = volume.clamp(0, 128) as u8;
        }
        self.audio_requests.push_back(AudioCommand::SetVolume {
            channel: native_se_audio_channel(channel),
            volume: native_audio_volume(volume),
            fade_ms: native_audio_duration(fade_ms),
        });
    }

    fn play_native_bgm(
        &mut self,
        channel: i32,
        archive: &str,
        file: &str,
        volume: i32,
        pan: i32,
    ) {
        let key = (archive.to_string(), file.to_string());
        if self.current_bgm.as_ref() == Some(&key) {
            return;
        }
                self.current_bgm = Some(key);
        let Some(entry) = find_runtime_resource(&self.manager, archive, file) else {
            tracing::warn!(channel, archive, file, "SoundLoadBgm resource missing");
            return;
        };
        let Ok(bytes) = self.manager.read_by_entry_decoded(&entry) else {
            tracing::warn!(channel, archive, file, "SoundLoadBgm read failed");
            return;
        };
        self.audio_requests.push_back(AudioCommand::Play {
            asset: AudioAsset {
                archive: archive.to_string(),
                file: file.to_string(),
                bytes,
            },
            channel,
            looped: true,
            volume: native_audio_volume(volume),
            decode_gain: 1.0,
            playback_rate: 1.0,
            panning: native_audio_panning(pan),
            fade_ms: 0,
            restart: true,
        });
        set_bgm_channel_active(&mut self.bgm_channel_active, channel, true);
        self.start_bgm_clock(channel, 1.0);
    }

    fn play_native_sound_slot(&mut self, channel: i32, volume: i32, pan: i32) {
        let Some(sound) = self.sound_slots.get(&channel).cloned() else {
            return;
        };
        self.audio_requests.push_back(AudioCommand::Play {
            asset: sound.asset,
            channel: native_se_audio_channel(channel),
            looped: sound.looped,
            volume: native_audio_volume(volume),
            decode_gain: sound.decode_gain,
            playback_rate: sound.playback_rate,
            panning: native_audio_panning(pan),
            fade_ms: sound.fade_in_ms,
            restart: true,
        });
        set_sound_channel_active(&mut self.sound_channel_active, channel, true);
        self.start_sound_clock(channel, sound.playback_rate);
    }

    pub(super) fn advance_audio_clocks(&mut self, elapsed_ms: u64) {
        for clock in &mut self.bgm_playback_clocks {
            clock.advance(elapsed_ms);
        }
        for clock in &mut self.sound_playback_clocks {
            clock.advance(elapsed_ms);
        }
    }

    fn start_bgm_clock(&mut self, channel: i32, playback_rate: f64) {
        if let Some(clock) = native_clock_mut(&mut self.bgm_playback_clocks, channel) {
            clock.start(playback_rate);
        }
    }

    fn set_bgm_clock_paused(&mut self, channel: i32, paused: bool) {
        if let Some(clock) = native_clock_mut(&mut self.bgm_playback_clocks, channel) {
            if paused {
                clock.pause();
            } else {
                clock.resume();
            }
        }
    }

    fn bgm_position(&self, channel: i32) -> i32 {
        native_clock(&self.bgm_playback_clocks, channel)
            .filter(|clock| clock.is_running())
            .map(NativeAudioClock::position_ms)
            .unwrap_or_default()
    }

    fn start_sound_clock(&mut self, channel: i32, playback_rate: f64) {
        if let Some(clock) = native_clock_mut(&mut self.sound_playback_clocks, channel) {
            clock.start(playback_rate);
        }
    }

    fn stop_sound_clock(&mut self, channel: i32) {
        if let Some(clock) = native_clock_mut(&mut self.sound_playback_clocks, channel) {
            clock.stop();
        }
    }

    fn sound_position(&self, channel: i32) -> i32 {
        native_clock(&self.sound_playback_clocks, channel)
            .map(NativeAudioClock::position_ms)
            .unwrap_or_default()
    }
}

fn native_audio_fixed(value: i32) -> f64 {
    f64::from(value) / 65_536.0
}

fn native_se_audio_channel(channel: i32) -> i32 {
    0x100 + channel
}

fn set_bgm_channel_active(active: &mut [bool; 16], channel: i32, value: bool) {
    if let Some(slot) = usize::try_from(channel)
        .ok()
        .and_then(|index| active.get_mut(index))
    {
        *slot = value;
    }
}

fn native_clock<const N: usize>(
    clocks: &[NativeAudioClock; N],
    channel: i32,
) -> Option<NativeAudioClock> {
    usize::try_from(channel)
        .ok()
        .and_then(|index| clocks.get(index))
        .copied()
}

fn native_clock_mut<const N: usize>(
    clocks: &mut [NativeAudioClock; N],
    channel: i32,
) -> Option<&mut NativeAudioClock> {
    usize::try_from(channel)
        .ok()
        .and_then(|index| clocks.get_mut(index))
}

#[cfg(test)]
mod tests {
    use super::{native_audio_fixed, native_se_audio_channel, OWNED_A0_IDS};

    #[test]
    fn owns_every_recovered_sound_dispatch_entry() {
        let registered = (0..=u8::MAX)
            .map(u16::from)
            .filter(|id| ethornell_script::native_abi::lookup(0xA0, *id).is_some())
            .collect::<Vec<_>>();
        assert_eq!(registered, OWNED_A0_IDS);
    }

    #[test]
    fn preserves_native_fixed_sound_fields_and_bank_separation() {
        assert_eq!(native_audio_fixed(0x0001_0000), 1.0);
        assert_eq!(native_audio_fixed(0x0010_0000), 16.0);
        assert_eq!(native_se_audio_channel(0), 0x100);
        assert_eq!(native_se_audio_channel(24), 0x118);
    }
}
