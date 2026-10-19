use sympoie::clip::{AudioClip, MidiClip, MidiNote};
use sympoie::clip_export::{audio_clip_project, write_midi_clip};
use sympoie::project::{AudioBuffer, Project};
use sympoie::render::Renderer;
use sympoie::time::{Tempo, TimeSignature};

fn note(start_tick: u64, duration_ticks: u64, key: u8) -> MidiNote {
    MidiNote {
        start_tick,
        duration_ticks,
        key,
        velocity: 90,
        channel: 2,
        slide_ticks: 0,
    }
}

type Events = Vec<(u64, Vec<u8>)>;

/// A minimal Standard MIDI File reader: (header fields, track events as
/// (absolute tick, bytes)).
fn parse(bytes: &[u8]) -> ((u16, u16, u16), Events) {
    assert_eq!(&bytes[..4], b"MThd");
    assert_eq!(u32::from_be_bytes(bytes[4..8].try_into().unwrap()), 6);
    let field = |at: usize| u16::from_be_bytes([bytes[at], bytes[at + 1]]);
    let header = (field(8), field(10), field(12));
    assert_eq!(&bytes[14..18], b"MTrk");
    let length = u32::from_be_bytes(bytes[18..22].try_into().unwrap()) as usize;
    let track = &bytes[22..];
    assert_eq!(track.len(), length);
    let mut at = 0;
    let mut tick = 0;
    let mut events = Vec::new();
    let read_vlq = |at: &mut usize| {
        let mut value = 0_u64;
        loop {
            let byte = track[*at];
            *at += 1;
            value = (value << 7) | u64::from(byte & 0x7f);
            if byte & 0x80 == 0 {
                return value;
            }
        }
    };
    while at < track.len() {
        tick += read_vlq(&mut at);
        let status = track[at];
        let event = if status == 0xff {
            let start = at;
            at += 2;
            let length = read_vlq(&mut at) as usize;
            at += length;
            track[start..at].to_vec()
        } else {
            at += 3;
            track[at - 3..at].to_vec()
        };
        events.push((tick, event));
    }
    (header, events)
}

#[test]
fn midi_clips_export_their_audible_notes_with_tempo_and_meter() {
    let clip = MidiClip {
        offset_ticks: 480,
        notes: vec![
            // Hidden before the clip's offset.
            note(0, 240, 40),
            note(480, 480, 60),
            // Repeated key: its note-off comes before the next note-on.
            note(960, 480, 60),
            // Cut at the clip's end.
            note(1200, 5000, 67),
        ],
        ..MidiClip::new(3840, 1920)
    };
    let mut bytes = Vec::new();
    write_midi_clip(
        &clip,
        "Verse",
        Tempo::new(90.0).unwrap(),
        TimeSignature::new(6, 8).unwrap(),
        &mut bytes,
    )
    .unwrap();
    let (header, events) = parse(&bytes);
    assert_eq!(header, (0, 1, 960));
    assert_eq!(events[0], (0, [&[0xff, 0x03, 5][..], b"Verse"].concat()));
    // 60,000,000 / 90 µs per quarter note.
    assert_eq!(events[1], (0, vec![0xff, 0x51, 3, 0x0a, 0x2c, 0x2b]));
    assert_eq!(events[2], (0, vec![0xff, 0x58, 4, 6, 3, 24, 8]));
    assert_eq!(
        events[3..],
        [
            (0, vec![0x92, 60, 90]),
            (480, vec![0x82, 60, 64]),
            (480, vec![0x92, 60, 90]),
            (720, vec![0x92, 67, 90]),
            (960, vec![0x82, 60, 64]),
            (1920, vec![0x82, 67, 64]),
            (1920, vec![0xff, 0x2f, 0]),
        ]
    );
}

#[test]
fn an_empty_midi_clip_still_lasts_its_length() {
    let mut bytes = Vec::new();
    write_midi_clip(
        &MidiClip::new(0, 200_000),
        "",
        Tempo::default(),
        TimeSignature::default(),
        &mut bytes,
    )
    .unwrap();
    let (_, events) = parse(&bytes);
    assert_eq!(events.last().unwrap(), &(200_000, vec![0xff, 0x2f, 0]));
}

#[test]
fn audio_clips_export_as_they_play() {
    let mut project = Project::new();
    project.set_bpm(97.0).unwrap();
    let source: Vec<_> = (0..20_000)
        .map(|i| {
            [
                (i as f32 * 0.01).sin() * 0.5,
                (i as f32 * 0.013).cos() * 0.4,
            ]
        })
        .collect();
    let asset = project.add_asset(AudioBuffer::new(source.clone()).unwrap());
    let track = project.add_audio_track("Vocal");
    project.add_audio_track("Other");
    let clip = AudioClip {
        asset,
        source_start: 1234,
        start_tick: 7777,
        frame_count: 9000,
        gain: -0.5,
        pitch_semitones: 0.0,
        pitch_algorithm: Default::default(),
        preserve_formants: false,
        name: Some("Take".into()),
    };
    project.add_audio_clip(track, clip.clone()).unwrap();

    let (isolated, frames) = audio_clip_project(&project, 1, 0).unwrap();
    assert_eq!(frames, 9000);
    let mut output = vec![[0.0; 2]; frames as usize];
    Renderer::new(&isolated)
        .unwrap()
        .render_block(0, &mut output);
    for (index, frame) in output.iter().enumerate() {
        let expected = source[1234 + index].map(|sample| sample * -0.5);
        assert!(
            (frame[0] - expected[0]).abs() < 1e-6 && (frame[1] - expected[1]).abs() < 1e-6,
            "frame {index}"
        );
    }

    // Pitched clips render through the same pitch processing as playback.
    let mut pitched = project.clone();
    pitched.set_audio_clip_pitch(track, 0, 3.0).unwrap();
    let (isolated, frames) = audio_clip_project(&pitched, 1, 0).unwrap();
    let _pins = isolated.prepare_audio_pitch(|| false).unwrap().unwrap();
    let mut shifted = vec![[0.0; 2]; frames as usize];
    Renderer::new(&isolated)
        .unwrap()
        .render_block(0, &mut shifted);
    let _pins = pitched.prepare_audio_pitch(|| false).unwrap().unwrap();
    let start = pitched.tempo().ticks_to_frames(7777).unwrap();
    let mut playing = vec![[0.0; 2]; (start + frames) as usize];
    Renderer::new(&pitched)
        .unwrap()
        .render_block(0, &mut playing);
    assert_eq!(shifted, playing[start as usize..]);
    assert_ne!(shifted, output);

    assert!(audio_clip_project(&project, 2, 0).is_err());
    assert!(audio_clip_project(&project, 1, 1).is_err());
}
