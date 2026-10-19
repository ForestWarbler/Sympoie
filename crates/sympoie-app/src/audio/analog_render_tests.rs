use super::*;
use sympoie::builtin;
use sympoie::clip::{BendPoint, MidiClip, MidiNote};
use sympoie::plugin::PluginInstance;
use sympoie::track::MidiTrackId;

fn ferro(engine: f64) -> PluginInstance {
    let mut instance = BuiltinKind::Ferro.instance();
    let spec = builtin::parameters(BuiltinKind::Ferro)
        .into_iter()
        .find(|spec| spec.id == ferro::ENGINE)
        .unwrap();
    instance
        .parameters
        .iter_mut()
        .find(|parameter| parameter.id == ferro::ENGINE)
        .unwrap()
        .value = spec.normalize(engine);
    instance
}

fn project(engine: f64) -> (Project, MidiTrackId, u64) {
    let mut project = Project::new();
    let track = project.add_midi_track("Ferro");
    project
        .set_plugin_instrument(track, Some(ferro(engine)))
        .unwrap();
    let mut clip = MidiClip::new(0, 1920);
    clip.notes.push(MidiNote::new(0, 480, 48, 100));
    clip.notes.push(MidiNote {
        slide_ticks: 240,
        ..MidiNote::new(480, 960, 60, 100)
    });
    clip.pitch_bend = vec![
        BendPoint {
            tick: 960,
            value: 0,
        },
        BendPoint {
            tick: 1440,
            value: 6000,
        },
    ];
    project.add_midi_clip(track, clip).unwrap();
    let id = project
        .midi_track(track)
        .unwrap()
        .plugin_instrument()
        .unwrap()
        .id;
    (project, track, id)
}

fn settle(renders: &mut AnalogRenders, plugins: &PluginRuntime) -> Poll {
    let deadline = Instant::now() + Duration::from_secs(60);
    let mut last = Poll::default();
    while renders.busy() {
        assert!(Instant::now() < deadline, "the render did not finish");
        let poll = renders.poll(plugins);
        if poll.changed || poll.failure.is_some() {
            last = poll;
        }
        thread::sleep(Duration::from_millis(5));
    }
    last
}

fn render(project: &Project, quality: RenderQuality, cached: &[(u64, Arc<[Frame]>)]) -> Vec<Frame> {
    let frames = project.duration_frames().unwrap();
    let mut renderer = Renderer::new(project).unwrap();
    renderer.set_quality(quality);
    renderer.set_instrument_renders(cached);
    let mut output = vec![[0.0; 2]; frames as usize];
    for (index, chunk) in output.chunks_mut(256).enumerate() {
        renderer.render_block(index as u64 * 256, chunk);
    }
    output
}

#[test]
fn live_engine_instruments_are_not_rendered() {
    let plugins = PluginRuntime::new();
    let (project, ..) = project(0.0);
    let mut renders = AnalogRenders::default();
    assert!(!renders.sync(&project, &plugins));
    assert!(!renders.busy());
    assert!(renders.renders().is_empty());
}

#[test]
fn cached_playback_matches_an_offline_render_of_the_project() {
    let plugins = PluginRuntime::new();
    let (project, _, id) = project(1.0);
    let mut renders = AnalogRenders::default();
    renders.sync(&project, &plugins);
    assert!(renders.busy());
    assert!(
        renders.renders().is_empty(),
        "nothing plays from cache before it exists"
    );
    let poll = settle(&mut renders, &plugins);
    assert!(poll.changed, "{:?}", poll.failure);
    let cached = renders.renders();
    assert_eq!(cached.len(), 1);
    assert_eq!(cached[0].0, id);
    assert!(
        cached[0]
            .1
            .iter()
            .flatten()
            .any(|sample| sample.abs() > 0.01)
    );

    // A realtime renderer playing the cache sounds exactly like an offline render.
    let offline = render(&project, RenderQuality::Offline, &[]);
    let from_cache = render(&project, RenderQuality::Realtime, &cached);
    let live = render(&project, RenderQuality::Realtime, &[]);
    assert_eq!(offline, from_cache);
    assert_ne!(offline, live);
}

#[test]
fn edits_retire_the_render_and_re_render() {
    let plugins = PluginRuntime::new();
    let (mut project, track, _) = project(1.0);
    let mut renders = AnalogRenders::default();
    renders.sync(&project, &plugins);
    settle(&mut renders, &plugins);
    let first = renders.renders();
    assert_eq!(first.len(), 1);

    // Unrelated edits keep the render.
    project.add_audio_track("Other");
    assert!(!renders.sync(&project, &plugins));
    assert!(!renders.busy());

    // Changing a slide makes the render stale: it stops playing at once.
    let mut notes = project.midi_track(track).unwrap().clips()[0].notes.clone();
    notes[1].slide_ticks = 60;
    project.set_midi_clip_notes(track, 0, notes).unwrap();
    assert!(renders.sync(&project, &plugins));
    assert!(renders.renders().is_empty());
    settle(&mut renders, &plugins);
    let second = renders.renders();
    assert_eq!(second.len(), 1);
    assert_ne!(first[0].1, second[0].1);

    // Back to Live: the render is dropped.
    let id = second[0].0;
    let spec = builtin::parameters(BuiltinKind::Ferro)
        .into_iter()
        .find(|spec| spec.id == ferro::ENGINE)
        .unwrap();
    project
        .set_plugin_parameter(id, ferro::ENGINE, spec.normalize(0.0))
        .unwrap();
    assert!(renders.sync(&project, &plugins));
    assert!(renders.renders().is_empty());
    assert!(!renders.busy());
}

#[test]
fn rapid_edits_share_one_render() {
    let plugins = PluginRuntime::new();
    let (mut project, track, _) = project(1.0);
    let mut renders = AnalogRenders::default();
    for key in 50..60 {
        let mut notes = project.midi_track(track).unwrap().clips()[0].notes.clone();
        notes[0].key = key;
        project.set_midi_clip_notes(track, 0, notes).unwrap();
        renders.sync(&project, &plugins);
        // Within the settle time nothing starts.
        assert!(renders.poll(&plugins) == Poll::default());
        assert!(renders.job.is_none());
    }
    settle(&mut renders, &plugins);
    assert_eq!(renders.renders().len(), 1);
}
