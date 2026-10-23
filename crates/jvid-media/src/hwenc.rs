//! VAAPI encoders take frames in GPU memory, so they get a hardware frame pool and each NV12
//! frame is uploaded before encoding. The only `unsafe` for encode lives here: ffmpeg-next has no
//! safe hwframe API (ADR 0001 amendment, issue #169), nor a way to fill frame side data, which
//! hardware HEVC encoders read HDR10 metadata from.
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
use ffmpeg_next::frame::side_data;
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
pub(crate) fn attach(ctx: &mut codec::Context, (width, height): (u32, u32)) -> bool {
    // SAFETY: `ctx` is exclusively borrowed and live. Refuse to overwrite an existing owned
    // reference, even before probing the device, so repeated calls cannot leak a frame pool.
    if unsafe { !(*ctx.as_ptr()).hw_frames_ctx.is_null() } {
        return false;
    }
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

/// Attaches HDR10 static metadata to `f`, which hardware encoders write as SEI: a P3 D65
/// mastering display of 0.0001–1000 cd/m², and `max_cll` as both MaxCLL and MaxFALL.
#[allow(unsafe_code)]
pub fn hdr10_metadata(f: &mut frame::Video, max_cll: u32) {
    // AVMasteringDisplayMetadata: R, G, B and white xy as rationals, min and max luminance, then
    // has_primaries and has_luminance. AVContentLightMetadata: MaxCLL, MaxFALL.
    let xy = |x, y| [x, 50_000, y, 50_000];
    let mut mdcv = [xy(34_000, 16_000), xy(13_250, 34_500), xy(7_500, 3_000)].concat();
    mdcv.extend(xy(15_635, 16_450));
    mdcv.extend([1, 10_000, 1_000, 1, 1, 1]);
    let bytes = |v: &[i32]| -> Vec<u8> { v.iter().flat_map(|n| n.to_ne_bytes()).collect() };
    let cll = max_cll as i32;
    for (kind, data) in [
        (side_data::Type::MasteringDisplayMetadata, bytes(&mdcv)),
        (side_data::Type::ContentLightLevel, bytes(&[cll, cll])),
    ] {
        if let Some(mut sd) = f.new_side_data(kind, data.len()) {
            // SAFETY: FFmpeg just allocated `data.len()` bytes for this side data, which `f` owns.
            unsafe { ptr::copy_nonoverlapping(data.as_ptr(), (*sd.as_mut_ptr()).data, data.len()) };
        }
    }
}

/// Proves that the opened pool can accept an actual NV12 frame before a muxer writes a header.
/// The probe is never sent to the encoder, so it cannot affect output timestamps or references.
pub(crate) fn probe_upload(ctx: &codec::Context, size: (u32, u32)) -> Result<(), Error> {
    probe_upload_with(ctx, size, upload)
}

fn probe_upload_with(
    ctx: &codec::Context,
    (width, height): (u32, u32),
    transfer: impl FnOnce(&codec::Context, &frame::Video) -> Result<frame::Video, Error>,
) -> Result<(), Error> {
    let mut nv12 = frame::Video::new(ffmpeg_next::format::Pixel::NV12, width, height);
    // Initialise padding as well as pixels: no uninitialised host data is sent to the driver.
    nv12.data_mut(0).fill(16);
    nv12.data_mut(1).fill(128);
    transfer(ctx, &nv12).map(drop)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hdr10_metadata_matches_the_libavutil_layout() {
        let mut f = frame::Video::new(ffmpeg_next::format::Pixel::P010LE, 4, 4);
        hdr10_metadata(&mut f, 203);
        let ints = |t| -> Vec<i32> {
            let sd = f.side_data(t).unwrap();
            let (chunks, rest) = sd.data().as_chunks::<4>();
            assert!(rest.is_empty());
            chunks.iter().map(|c| i32::from_ne_bytes(*c)).collect()
        };
        let mdcv = ints(side_data::Type::MasteringDisplayMetadata);
        // 10 rationals and 2 ints; max luminance is 1000/1 cd/m².
        assert_eq!(mdcv.len(), 22);
        assert_eq!(mdcv[18..], [1000, 1, 1, 1]);
        assert_eq!(ints(side_data::Type::ContentLightLevel), [203, 203]);
    }

    #[test]
    fn upload_without_a_pool_fails() {
        crate::ffmpeg::init().unwrap();
        let c = ffmpeg_next::encoder::find_by_name("mpeg4").unwrap();
        let ctx = codec::Context::new_with_codec(c);
        let f = frame::Video::new(ffmpeg_next::format::Pixel::NV12, 4, 4);
        assert!(upload(&ctx, &f).is_err());
    }

    #[test]
    fn startup_upload_failure_is_propagated() {
        crate::ffmpeg::init().unwrap();
        let ctx = codec::Context::new();
        let result = probe_upload_with(&ctx, (64, 32), |_, f| {
            assert_eq!(f.format(), ffmpeg_next::format::Pixel::NV12);
            assert_eq!((f.width(), f.height()), (64, 32));
            assert!(f.data(0).iter().all(|b| *b == 16));
            assert!(f.data(1).iter().all(|b| *b == 128));
            Err(Error::External)
        });
        assert_eq!(result, Err(Error::External));
    }

    #[test]
    #[allow(unsafe_code)]
    fn attach_preserves_an_existing_reference_without_a_device() {
        let mut ctx = codec::Context::new();
        // SAFETY: a fresh generic AVBufferRef is sufficient here: attach must reject it without
        // accessing its data as a frame pool. Ownership passes to ctx, whose drop unrefs it.
        let reference = unsafe { ffmpeg_next::ffi::av_buffer_alloc(1) };
        assert!(!reference.is_null());
        // SAFETY: ctx is exclusively borrowed and has no existing reference.
        unsafe {
            (*ctx.as_mut_ptr()).hw_frames_ctx = reference;
        }
        assert!(!attach(&mut ctx, (64, 64)));
        // SAFETY: ctx still owns the live reference; read only, before ctx is dropped.
        assert_eq!(unsafe { (*ctx.as_ptr()).hw_frames_ctx }, reference);
    }
}
