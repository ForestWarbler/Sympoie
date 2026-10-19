//! Exporting a single clip: MIDI clips as Standard MIDI Files and audio clips
//! as the audio they play.

use std::io::{self, Write};

use crate::TICKS_PER_QUARTER;
use crate::clip::MidiClip;
use crate::project::Project;
use crate::time::{Tempo, TimeSignature};
use crate::track::Track;

/// Writes `clip` as a format-0 Standard MIDI File at 960 ticks per quarter
/// note, with the project's tempo and meter. Only the notes the clip plays are
/// written, measured from the clip's start, and the file lasts as long as the
/// clip.
pub fn write_midi_clip(
    clip: &MidiClip,
    name: &str,
    tempo: Tempo,
    meter: TimeSignature,
    mut writer: impl Write,
) -> io::Result<()> {
    let mut events: Vec<(u64, u8, [u8; 3])> = Vec::new();
    for note in clip.audible_notes() {
        let channel = note.channel & 0x0f;
        let key = note.key & 0x7f;
        events.push((
            note.start_tick,
            2,
            [0x90 | channel, key, note.velocity.clamp(1, 127)],
        ));
        events.push((
            note.start_tick + note.duration_ticks,
            0,
            [0x80 | channel, key, 64],
        ));
    }
    let bend = clip.audible_bend();
    if !bend.is_empty() {
        let mut channels: Vec<u8> = clip.audible_notes().map(|n| n.channel & 0x0f).collect();
        channels.sort_unstable();
        channels.dedup();
        if channels.is_empty() {
            channels.push(0);
        }
        // Ramps between breakpoints become a message every 1/96 quarter note.
        const STEP: u64 = TICKS_PER_QUARTER / 96;
        let mut ticks: Vec<u64> = Vec::new();
        for pair in bend.windows(2) {
            let (from, to) = (pair[0], pair[1]);
            ticks.push(from.tick);
            if from.value != to.value {
                ticks.extend((from.tick + STEP..to.tick).step_by(STEP as usize));
            }
        }
        ticks.extend(bend.last().map(|point| point.tick));
        let mut last = None;
        for tick in ticks.into_iter().filter(|tick| *tick < clip.length_ticks) {
            let raw = (i32::from(crate::clip::bend_value_at(&bend, tick)) + 8192) as u16;
            if last == Some(raw) {
                continue;
            }
            last = Some(raw);
            for &channel in &channels {
                events.push((
                    tick,
                    1,
                    [0xe0 | channel, (raw & 0x7f) as u8, (raw >> 7) as u8],
                ));
            }
        }
        if last.is_some_and(|raw| raw != 8192) {
            for &channel in &channels {
                events.push((clip.length_ticks, 1, [0xe0 | channel, 0, 64]));
            }
        }
    }
    // At the same tick, notes end, then bends move, then new notes start, so repeated
    // keys retrigger at the right pitch.
    events.sort_by_key(|(tick, order, _)| (*tick, *order));

    let mut track = Vec::new();
    let meta = |track: &mut Vec<u8>, kind: u8, data: &[u8]| {
        track.push(0);
        track.extend_from_slice(&[0xff, kind]);
        variable_length(track, data.len() as u64);
        track.extend_from_slice(data);
    };
    meta(&mut track, 0x03, name.as_bytes());
    let micros = (60_000_000.0 / tempo.bpm())
        .round()
        .clamp(1.0, f64::from(0xff_ffff)) as u32;
    meta(&mut track, 0x51, &micros.to_be_bytes()[1..]);
    meta(
        &mut track,
        0x58,
        &[
            meter.numerator().min(255) as u8,
            meter.denominator().trailing_zeros() as u8,
            24,
            8,
        ],
    );
    let mut last = 0;
    for (tick, _, message) in events {
        variable_length(&mut track, tick - last);
        track.extend_from_slice(&message);
        last = tick;
    }
    let end = clip.length_ticks.max(last);
    variable_length(&mut track, end - last);
    track.extend_from_slice(&[0xff, 0x2f, 0x00]);

    let length = u32::try_from(track.len())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "the MIDI clip is too long"))?;
    writer.write_all(b"MThd")?;
    writer.write_all(&6_u32.to_be_bytes())?;
    writer.write_all(&0_u16.to_be_bytes())?;
    writer.write_all(&1_u16.to_be_bytes())?;
    writer.write_all(&(TICKS_PER_QUARTER as u16).to_be_bytes())?;
    writer.write_all(b"MTrk")?;
    writer.write_all(&length.to_be_bytes())?;
    writer.write_all(&track)?;
    writer.flush()
}

fn variable_length(out: &mut Vec<u8>, value: u64) {
    let mut bytes = vec![(value & 0x7f) as u8];
    let mut rest = value >> 7;
    while rest != 0 {
        bytes.push(0x80 | (rest & 0x7f) as u8);
        rest >>= 7;
    }
    out.extend(bytes.iter().rev());
}

/// A project that plays only the given audio clip, from frame zero, through a
/// neutral mixer: rendering its first `frame_count` frames yields the clip's
/// audio as it plays, with its trim, gain and pitch settings applied.
pub fn audio_clip_project(
    project: &Project,
    track: usize,
    clip: usize,
) -> Result<(Project, u64), String> {
    let Some(Track::Audio(source)) = project.tracks().get(track) else {
        return Err("The clip's track is not an audio track.".into());
    };
    let mut clip = source
        .clips()
        .get(clip)
        .ok_or("The clip no longer exists.")?
        .clone();
    let mut isolated = Project::new();
    isolated
        .set_bpm(project.bpm())
        .map_err(|error| error.to_string())?;
    let asset = project
        .shared_audio_asset(clip.asset)
        .map_err(|error| error.to_string())?;
    clip.asset = isolated.add_shared_asset(asset);
    clip.start_tick = 0;
    let frame_count = clip.frame_count;
    let id = isolated.add_audio_track(source.name());
    isolated
        .add_audio_clip(id, clip)
        .map_err(|error| error.to_string())?;
    Ok((isolated, frame_count))
}
