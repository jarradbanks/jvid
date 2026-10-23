//! VAAPI encoders take frames in GPU memory, so they get a hardware frame pool and each NV12
//! frame is uploaded before encoding. The only `unsafe` for encode lives here: ffmpeg-next has no
//! safe hwframe API (ADR 0001 amendment, issue #169).
//!
//! Ownership: the device ref lives in a static for the whole process. `attach` creates a frames
//! context holding its own device ref and hands that one frames ref to the codec context, which
//! unrefs it in `avcodec_free_context` (when the `encoder::Video` drops). `upload` returns a
//! `frame::Video` owning a pool surface; the frame keeps the pool alive and returns the surface
//! when it drops, after `send_frame` has taken its own ref.

use std::ptr;
use std::sync::OnceLock;

use ffmpeg_next::ffi::{
    AVBufferRef, AVHWDeviceType, AVHWFramesContext, AVPixelFormat, av_buffer_unref,
    av_frame_copy_props, av_hwdevice_ctx_create, av_hwframe_ctx_alloc, av_hwframe_ctx_init,
    av_hwframe_get_buffer, av_hwframe_transfer_data,
};
use ffmpeg_next::{Error, codec, frame};

/// True for encoders that need [`attach`] and [`upload`].
pub fn is_vaapi(name: &str) -> bool {
    name.ends_with("_vaapi")
}

/// A device context ref kept for the whole process.
struct Device(*mut AVBufferRef);
// SAFETY: FFmpeg device contexts are thread-safe to share; the pointer is never freed or mutated.
#[allow(unsafe_code)]
unsafe impl Send for Device {}
// SAFETY: as above.
#[allow(unsafe_code)]
unsafe impl Sync for Device {}

/// The default VAAPI render node, opened once; `None` without one.
#[allow(unsafe_code)]
fn device() -> Option<&'static Device> {
    static DEVICE: OnceLock<Option<Device>> = OnceLock::new();
    DEVICE
        .get_or_init(|| {
            let mut device = ptr::null_mut();
            // SAFETY: `device` is a valid out-pointer; null device name and options ask for the
            // default.
            let r = unsafe {
                av_hwdevice_ctx_create(
                    &mut device,
                    AVHWDeviceType::AV_HWDEVICE_TYPE_VAAPI,
                    ptr::null(),
                    ptr::null_mut(),
                    0,
                )
            };
            if r < 0 {
                tracing::debug!(error = %Error::from(r), "vaapi device unavailable");
                return None;
            }
            Some(Device(device))
        })
        .as_ref()
}

/// Gives the unopened encoder `ctx` a pool of `width`×`height` NV12-backed VAAPI surfaces.
/// Returns false (leaving `ctx` untouched) without a device or when the pool fails.
#[allow(unsafe_code)]
pub fn attach(ctx: &mut codec::Context, (width, height): (u32, u32)) -> bool {
    let Some(device) = device() else {
        return false;
    };
    // SAFETY: `device.0` is a live device ref; the new frames context takes its own ref to it.
    let mut frames = unsafe { av_hwframe_ctx_alloc(device.0) };
    if frames.is_null() {
        return false;
    }
    // SAFETY: `frames` is a fresh, uninitialised frames ref, so `data` is its AVHWFramesContext
    // and we're its only user. On failure we unref it, freeing the context and its device ref.
    unsafe {
        let fc = (*frames).data.cast::<AVHWFramesContext>();
        (*fc).format = AVPixelFormat::AV_PIX_FMT_VAAPI;
        (*fc).sw_format = AVPixelFormat::AV_PIX_FMT_NV12;
        (*fc).width = width as i32;
        (*fc).height = height as i32;
        // Encoders keep a few reference surfaces besides the frame in flight.
        (*fc).initial_pool_size = 20;
        let r = av_hwframe_ctx_init(frames);
        if r < 0 {
            tracing::debug!(error = %Error::from(r), "vaapi frame pool failed");
            av_buffer_unref(&mut frames);
            return false;
        }
        // The context is not opened yet and has no frames context; it takes ownership of our
        // only ref and unrefs it when freed.
        (*ctx.as_mut_ptr()).hw_frames_ctx = frames;
    }
    true
}

/// Copies the NV12 frame `f` into a surface from the pool `ctx` got from [`attach`], keeping pts.
#[allow(unsafe_code)]
pub fn upload(ctx: &codec::Context, f: &frame::Video) -> Result<frame::Video, Error> {
    // SAFETY: `ctx` is valid for the call; we only read the pointer.
    let frames = unsafe { (*ctx.as_ptr()).hw_frames_ctx };
    // av_hwframe_get_buffer dereferences it unchecked.
    if frames.is_null() {
        return Err(Error::InvalidData);
    }
    let mut out = frame::Video::empty();
    // SAFETY: `frames` is owned by `ctx`, which outlives the call; `out` is empty, so it gets a
    // new ref to a pool surface (and the pool) that `out` frees on drop.
    let r = unsafe { av_hwframe_get_buffer(frames, out.as_mut_ptr(), 0) };
    if r < 0 {
        return Err(Error::from(r));
    }
    // SAFETY: both frames are valid and owned; `out` is a surface of the same size as `f`.
    let r = unsafe { av_hwframe_transfer_data(out.as_mut_ptr(), f.as_ptr(), 0) };
    if r < 0 {
        return Err(Error::from(r));
    }
    // SAFETY: as above; copies metadata only.
    let r = unsafe { av_frame_copy_props(out.as_mut_ptr(), f.as_ptr()) };
    if r < 0 {
        return Err(Error::from(r));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upload_without_a_pool_fails() {
        crate::ffmpeg::init().unwrap();
        let c = ffmpeg_next::encoder::find_by_name("mpeg4").unwrap();
        let ctx = codec::Context::new_with_codec(c);
        let f = frame::Video::new(ffmpeg_next::format::Pixel::NV12, 4, 4);
        assert!(upload(&ctx, &f).is_err());
    }
}
