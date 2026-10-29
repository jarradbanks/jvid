//! Shared SDR YUV-to-RGB preview policy. Output RGB is always full range.

use ffmpeg_next::{Error, software::scaling};

/// AREA integrates source pixels when shrinking; bilinear interpolates when enlarging.
/// Keep the unscaled path cheap (there is no minification to antialias).
pub fn scale_flags(source: (u32, u32), output: (u32, u32)) -> scaling::Flags {
    if output.0 < source.0 || output.1 < source.1 {
        scaling::Flags::AREA
    } else if output != source {
        scaling::Flags::BILINEAR
    } else {
        scaling::Flags::FAST_BILINEAR
    }
}

/// Apply the decoded SDR frame's matrix and range to a live RGB output scaler.
#[allow(unsafe_code)]
pub fn rgb_colorspace(scaler: &mut scaling::Context, bt709: bool, full: bool) -> Result<(), Error> {
    use ffmpeg_next::ffi::*;
    // SAFETY: the scaler owns the live context, and FFmpeg owns the static coefficient table.
    // Full-range RGB output uses neutral brightness, contrast and saturation.
    let result = unsafe {
        let table = sws_getCoefficients(if bt709 { SWS_CS_ITU709 } else { SWS_CS_ITU601 });
        sws_setColorspaceDetails(
            scaler.as_mut_ptr(),
            table,
            i32::from(full),
            table,
            1,
            0,
            1 << 16,
            1 << 16,
        )
    };
    if result < 0 {
        Err(Error::from(result))
    } else {
        Ok(())
    }
}
