use super::*;

#[test]
#[ignore = "requires a writable copy of a game via ETHORNELL_REPRO_GAME"]
fn game_input_repro() {
    ethornell_core::init_tracing();
    let root = PathBuf::from(std::env::var_os("ETHORNELL_REPRO_GAME").expect("game copy"));
    let manager = ResourceManager::open_game(&root).unwrap();
    let bytes = manager.read_decoded_from_archive("system.arc", "ipl._bp").unwrap();
    let mut runtime = RuntimeEngine::new("system.arc:ipl._bp", &bytes, manager, root, false, false, false);
    let mut script = HeadlessInputScript::from_env();
    let mut events = VecDeque::new();
    let pacing = RuntimeFramePacing::from_env();
    let frames = parse_usize_env("ETHORNELL_HEADLESS_FRAMES").unwrap_or(1000);
    for frame in 0..frames {
        let report = runtime_frontend::drive_runtime_frame(&mut runtime, &mut events, &mut script, pacing, NATIVE_TICK_MS, NATIVE_TICK_MS, None, "repro");
        if let Some(report) = report.last_report.as_ref() {
            assert!(!report.stop_reason.is_fatal(), "frame {frame}: {report:?}");
        }
        if frame % 100 == 0 { eprintln!("repro frame {frame}"); }
    }
    if let Some(path) = std::env::var_os("ETHORNELL_HEADLESS_SNAPSHOT") {
        snapshot::write_runtime_snapshot(&runtime.api, &PathBuf::from(path)).unwrap();
    }
    for (id, input) in &runtime.api.graph_input_objects {
        eprintln!("INPUT {id}: {input:?}");
    }
}
