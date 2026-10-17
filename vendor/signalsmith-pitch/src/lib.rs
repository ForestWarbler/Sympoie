use std::ffi::{c_char, c_int, c_void};
use std::ptr::NonNull;

const CHUNK: usize = 1024;
type Frame = [f32; 2];

unsafe extern "C" {
    fn sympoie_ss_create(
        sample_rate: u32,
        semitones: f32,
        preserve_formants: c_int,
        seek_frames: *mut c_int,
        tail_frames: *mut c_int,
        error: *mut c_char,
        capacity: usize,
    ) -> *mut c_void;
    fn sympoie_ss_prime(
        handle: *mut c_void,
        input: *const f32,
        available: usize,
        seek_frames: c_int,
        error: *mut c_char,
        capacity: usize,
    ) -> c_int;
    fn sympoie_ss_process(
        handle: *mut c_void,
        input: *const f32,
        available: usize,
        output: *mut f32,
        frames: c_int,
        error: *mut c_char,
        capacity: usize,
    ) -> c_int;
    fn sympoie_ss_flush(
        handle: *mut c_void,
        output: *mut f32,
        frames: c_int,
        error: *mut c_char,
        capacity: usize,
    ) -> c_int;
    fn sympoie_ss_destroy(handle: *mut c_void);
}

struct Native(NonNull<c_void>);
impl Drop for Native {
    fn drop(&mut self) {
        unsafe { sympoie_ss_destroy(self.0.as_ptr()) };
    }
}

fn message(error: &[c_char]) -> String {
    let bytes: Vec<u8> = error
        .iter()
        .take_while(|&&v| v != 0)
        .map(|&v| v as u8)
        .collect();
    String::from_utf8_lossy(&bytes).into_owned()
}

pub fn shift(
    samples: &[Frame],
    sample_rate: u32,
    semitones: f32,
    preserve_formants: bool,
    cancelled: &dyn Fn() -> bool,
) -> Result<Option<Vec<Frame>>, String> {
    if cancelled() {
        return Ok(None);
    }
    if !(1_000..=768_000).contains(&sample_rate) {
        return Err("Signalsmith Stretch requires a sample rate between 1000 and 768000 Hz".into());
    }
    if !semitones.is_finite() || semitones.abs() > 96.0 {
        return Err("Signalsmith Stretch requires a finite pitch within ±96 semitones".into());
    }
    for block in samples.chunks(CHUNK) {
        if cancelled() {
            return Ok(None);
        }
        if block.iter().flatten().any(|sample| !sample.is_finite()) {
            return Err("Signalsmith Stretch input contains a non-finite sample".into());
        }
    }
    let mut output = Vec::new();
    output
        .try_reserve_exact(samples.len())
        .map_err(|e| format!("Signalsmith Stretch output allocation failed: {e}"))?;
    if samples.is_empty() {
        return Ok(Some(output));
    }
    if semitones == 0.0 {
        for block in samples.chunks(CHUNK) {
            if cancelled() {
                return Ok(None);
            }
            output.extend_from_slice(block);
        }
        return Ok((!cancelled()).then_some(output));
    }
    let mut error = [0 as c_char; 512];
    let mut seek_frames = 0;
    let mut tail_frames = 0;
    let handle = unsafe {
        sympoie_ss_create(
            sample_rate,
            semitones,
            preserve_formants.into(),
            &mut seek_frames,
            &mut tail_frames,
            error.as_mut_ptr(),
            error.len(),
        )
    };
    let native = Native(NonNull::new(handle).ok_or_else(|| message(&error))?);
    if seek_frames <= 0 || tail_frames <= 0 {
        return Err("Signalsmith Stretch returned invalid processing latencies".into());
    }
    if cancelled() {
        return Ok(None);
    }
    let ok = unsafe {
        sympoie_ss_prime(
            native.0.as_ptr(),
            samples.as_ptr().cast(),
            samples.len().min(seek_frames as usize),
            seek_frames,
            error.as_mut_ptr(),
            error.len(),
        )
    };
    if ok == 0 {
        return Err(message(&error));
    }
    let tail = (tail_frames as usize).min(samples.len());
    let process_frames = samples.len() - tail;
    let mut buffer = [[0.0; 2]; CHUNK];
    while output.len() < process_frames {
        if cancelled() {
            return Ok(None);
        }
        let count = CHUNK.min(process_frames - output.len());
        let source_position = (seek_frames as usize)
            .saturating_add(output.len())
            .min(samples.len());
        let input = &samples[source_position..];
        let ok = unsafe {
            sympoie_ss_process(
                native.0.as_ptr(),
                input.as_ptr().cast(),
                input.len().min(count),
                buffer.as_mut_ptr().cast(),
                count as c_int,
                error.as_mut_ptr(),
                error.len(),
            )
        };
        if ok == 0 {
            return Err(message(&error));
        }
        output.extend_from_slice(&buffer[..count]);
    }
    if cancelled() {
        return Ok(None);
    }
    let offset = output.len();
    output.resize(samples.len(), [0.0; 2]);
    let ok = unsafe {
        sympoie_ss_flush(
            native.0.as_ptr(),
            output[offset..].as_mut_ptr().cast(),
            tail as c_int,
            error.as_mut_ptr(),
            error.len(),
        )
    };
    if ok == 0 {
        return Err(message(&error));
    }
    for block in output.chunks(CHUNK) {
        if cancelled() {
            return Ok(None);
        }
        if block.iter().flatten().any(|sample| !sample.is_finite()) {
            return Err("Signalsmith Stretch produced a non-finite sample".into());
        }
    }
    Ok(Some(output))
}
