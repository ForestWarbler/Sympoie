use std::ffi::{c_char, c_int, c_uint, c_void};
use std::ptr::NonNull;

const BLOCK: usize = 1024;
const ERROR_CAPACITY: usize = 512;

unsafe extern "C" {
    fn sympoie_rb3_create(
        sample_rate: c_uint,
        pitch_scale: f64,
        preserve_formants: c_int,
        duration: usize,
        block_size: usize,
        error: *mut c_char,
        capacity: usize,
    ) -> *mut c_void;
    fn sympoie_rb3_feed(
        handle: *mut c_void,
        left: *const f32,
        right: *const f32,
        count: usize,
        final_block: c_int,
        study: c_int,
        error: *mut c_char,
        capacity: usize,
    ) -> c_int;
    fn sympoie_rb3_retrieve(
        handle: *mut c_void,
        left: *mut f32,
        right: *mut f32,
        max_count: usize,
        error: *mut c_char,
        capacity: usize,
    ) -> c_int;
    fn sympoie_rb3_destroy(handle: *mut c_void);
}

struct Engine(NonNull<c_void>);

impl Drop for Engine {
    fn drop(&mut self) {
        unsafe { sympoie_rb3_destroy(self.0.as_ptr()) };
    }
}

fn error_message(buffer: &[c_char; ERROR_CAPACITY]) -> String {
    let bytes: Vec<u8> = buffer
        .iter()
        .take_while(|&&byte| byte != 0)
        .map(|&byte| byte as u8)
        .collect();
    format!("Rubber Band R3: {}", String::from_utf8_lossy(&bytes))
}

pub fn shift(
    samples: &[[f32; 2]],
    sample_rate: u32,
    semitones: f32,
    preserve_formants: bool,
    cancelled: &dyn Fn() -> bool,
) -> Result<Option<Vec<[f32; 2]>>, String> {
    if cancelled() {
        return Ok(None);
    }
    if !(8_000..=192_000).contains(&sample_rate) {
        return Err("Rubber Band R3 requires a sample rate between 8000 and 192000 Hz".into());
    }
    if !semitones.is_finite() || !(-24.0..=24.0).contains(&semitones) {
        return Err("Rubber Band R3 pitch must be finite and between -24 and 24 semitones".into());
    }
    let mut output = Vec::new();
    output
        .try_reserve_exact(samples.len())
        .map_err(|e| format!("Rubber Band R3 could not allocate output: {e}"))?;
    if samples.is_empty() || semitones == 0.0 {
        for chunk in samples.chunks(BLOCK) {
            if cancelled() {
                return Ok(None);
            }
            validate_input(chunk)?;
            output.extend_from_slice(chunk);
        }
        return Ok((!cancelled()).then_some(output));
    }

    let minimum_input_frames = (sample_rate as usize)
        .div_ceil(32)
        .next_power_of_two()
        .max(1024)
        * 4
        + BLOCK;
    let processing_len = samples.len().max(minimum_input_frames);
    let mut retrieved_frames = 0;
    let mut error = [0 as c_char; ERROR_CAPACITY];
    let handle = unsafe {
        sympoie_rb3_create(
            sample_rate,
            2.0_f64.powf(f64::from(semitones) / 12.0),
            c_int::from(preserve_formants),
            processing_len,
            BLOCK,
            error.as_mut_ptr(),
            ERROR_CAPACITY,
        )
    };
    let engine = Engine(NonNull::new(handle).ok_or_else(|| error_message(&error))?);
    let mut left = [0.0_f32; BLOCK];
    let mut right = [0.0_f32; BLOCK];
    for study in [true, false] {
        for start in (0..processing_len).step_by(BLOCK) {
            if cancelled() {
                return Ok(None);
            }
            let end = (start + BLOCK).min(processing_len);
            let chunk = &samples[start.min(samples.len())..end.min(samples.len())];
            if study {
                validate_input(chunk)?;
            }
            left.fill(0.0);
            right.fill(0.0);
            for (i, frame) in chunk.iter().enumerate() {
                left[i] = frame[0];
                right[i] = frame[1];
            }
            let final_block = end == processing_len;
            let status = unsafe {
                sympoie_rb3_feed(
                    engine.0.as_ptr(),
                    left.as_ptr(),
                    right.as_ptr(),
                    end - start,
                    c_int::from(final_block),
                    c_int::from(study),
                    error.as_mut_ptr(),
                    ERROR_CAPACITY,
                )
            };
            if status != 0 {
                return Err(error_message(&error));
            }
            if study {
                continue;
            }
            loop {
                if cancelled() {
                    return Ok(None);
                }
                let count = unsafe {
                    sympoie_rb3_retrieve(
                        engine.0.as_ptr(),
                        left.as_mut_ptr(),
                        right.as_mut_ptr(),
                        BLOCK,
                        error.as_mut_ptr(),
                        ERROR_CAPACITY,
                    )
                };
                match count {
                    -2 => return Err(error_message(&error)),
                    -1 => break,
                    0 if final_block => {
                        return Err(
                            "Rubber Band R3 did not finish processing the final block".into()
                        );
                    }
                    0 => break,
                    1..=1024 => {
                        let count = count as usize;
                        if retrieved_frames + count > processing_len {
                            return Err("Rubber Band R3 returned more frames than requested".into());
                        }
                        for i in 0..count {
                            let frame = [left[i], right[i]];
                            if !frame[0].is_finite() || !frame[1].is_finite() {
                                return Err("Rubber Band R3 returned non-finite audio".into());
                            }
                            if retrieved_frames + i < samples.len() {
                                output.push(frame);
                            }
                        }
                        retrieved_frames += count;
                    }
                    _ => return Err("Rubber Band R3 returned an invalid frame count".into()),
                }
            }
        }
    }
    if cancelled() {
        return Ok(None);
    }
    if output.len() != samples.len() || retrieved_frames != processing_len {
        return Err(format!(
            "Rubber Band R3 returned {} frames, expected {}",
            retrieved_frames, processing_len
        ));
    }
    Ok(Some(output))
}

fn validate_input(samples: &[[f32; 2]]) -> Result<(), String> {
    if samples.iter().flatten().any(|value| !value.is_finite()) {
        Err("Rubber Band R3 input contains non-finite audio".into())
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests;
