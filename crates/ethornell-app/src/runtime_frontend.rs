use super::*;

pub(crate) fn drive_runtime_frame(
    runtime: &mut RuntimeEngine,
    pending_input_events: &mut VecDeque<RuntimeInputEvent>,
    input_script: &mut Option<HeadlessInputScript>,
    pacing: RuntimeFramePacing,
    elapsed_ms: u64,
    audio_elapsed_ms: u64,
    audio: Option<&mut AudioSystem>,
    frontend: &'static str,
) -> RuntimeFrameReport {
    // The target message pump processes the host messages accumulated before
    // the native tick. Consuming only one message per redraw can leave a
    // MouseDown/MouseUp edge stranded behind pointer motion for several VM
    // passes, so preserve FIFO order while draining the complete host queue.
    while let Some(event) = pending_input_events.pop_front() {
        apply_runtime_input_event(&mut runtime.api, event);
    }
    if let Some(script) = input_script.as_mut() {
        if let Some(event) = script.tick() {
            apply_headless_input_event(&mut runtime.api, event);
        }
    }

    let report = runtime.run_frame(pacing, elapsed_ms, audio_elapsed_ms);
    let mut audio = audio;
    while let Some(request) = runtime.api.audio_requests.pop_front() {
        execute_audio_command(audio.as_deref_mut(), request, frontend);
    }
    report
}
