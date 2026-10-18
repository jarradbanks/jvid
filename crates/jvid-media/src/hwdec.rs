//! Hardware video decoding through FFmpeg hwaccel, falling back to software when no device opens.
//! The only `unsafe` for decode lives here: ffmpeg-next has no safe hwaccel API (ADR 0001 amendment).

use std::ptr;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

use ffmpeg_next::ffi::{
    AVBufferRef, AVHWDeviceType, av_buffer_ref, av_frame_copy_props, av_hwdevice_ctx_create,
    av_hwframe_transfer_data,
};
use ffmpeg_next::{Error, codec, frame};

static ENABLED: AtomicBool = AtomicBool::new(true);

/// The `hardware_decode` setting; decoders opened afterwards follow it.
pub fn set_enabled(on: bool) {
    ENABLED.store(on, Ordering::Relaxed);
}

/// Device types to try, best first.
const DEVICES: &[(AVHWDeviceType, &str)] = if cfg!(windows) {
    &[
        (AVHWDeviceType::AV_HWDEVICE_TYPE_D3D11VA, "d3d11va"),
        (AVHWDeviceType::AV_HWDEVICE_TYPE_CUDA, "cuda"),
    ]
} else if cfg!(target_os = "macos") {
    &[(
        AVHWDeviceType::AV_HWDEVICE_TYPE_VIDEOTOOLBOX,
        "videotoolbox",
    )]
} else {
    &[
        (AVHWDeviceType::AV_HWDEVICE_TYPE_VAAPI, "vaapi"),
        (AVHWDeviceType::AV_HWDEVICE_TYPE_CUDA, "cuda"),
    ]
};

/// Test hook: when set, `attach` reports a fake device (attaching nothing) and decoders using it
/// fail at this stage, so the software fallback paths run without a GPU.
#[cfg(test)]
pub(crate) static FAKE_FAIL: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);
#[cfg(test)]
pub(crate) const FAKE: &str = "fake";
#[cfg(test)]
pub(crate) const FAIL_OPEN: u8 = 1;
#[cfg(test)]
pub(crate) const FAIL_DECODE: u8 = 2;
#[cfg(test)]
pub(crate) const FAIL_DOWNLOAD: u8 = 3;

/// A device context ref kept for the whole process and shared by every decoder.
struct Device(*mut AVBufferRef, &'static str);
// SAFETY: FFmpeg device contexts are thread-safe to share; the pointer is never freed or mutated.
#[allow(unsafe_code)]
unsafe impl Send for Device {}
// SAFETY: as above.
#[allow(unsafe_code)]
unsafe impl Sync for Device {}

/// The first device type that opens, created once so pooled decoders don't each take a GPU session.
#[allow(unsafe_code)]
fn device() -> Option<&'static Device> {
    static DEVICE: OnceLock<Option<Device>> = OnceLock::new();
    DEVICE
        .get_or_init(|| {
            DEVICES.iter().find_map(|&(kind, name)| {
                let mut device = ptr::null_mut();
                // SAFETY: `device` is a valid out-pointer; null device name and options ask for
                // the default.
                let r = unsafe {
                    av_hwdevice_ctx_create(&mut device, kind, ptr::null(), ptr::null_mut(), 0)
                };
                if r < 0 {
                    tracing::debug!(device = name, error = %Error::from(r), "hw device unavailable");
                    return None;
                }
                Some(Device(device, name))
            })
        })
        .as_ref()
}

/// Gives `ctx` a ref to the shared hardware device, returning its name. Call before opening the
/// decoder. If the codec or profile has no hwaccel for the device, FFmpeg's default `get_format`
/// picks a software format, so decoding still works.
#[allow(unsafe_code)]
pub fn attach(ctx: &mut codec::Context) -> Option<&'static str> {
    if !ENABLED.load(Ordering::Relaxed) {
        return None;
    }
    #[cfg(test)]
    if FAKE_FAIL.load(Ordering::Relaxed) != 0 {
        return Some(FAKE);
    }
    if !matches!(ctx.id(), codec::Id::H264 | codec::Id::HEVC) {
        return None;
    }
    let device = device()?;
    // SAFETY: `device.0` is a live device ref owned by the static; this takes a new ref to it.
    let r = unsafe { av_buffer_ref(device.0) };
    if r.is_null() {
        return None;
    }
    // SAFETY: the context is not opened yet and has no device; it takes ownership of the new ref
    // and unrefs it when freed.
    unsafe { (*ctx.as_mut_ptr()).hw_device_ctx = r };
    Some(device.1)
}

/// True when `f` lives in GPU memory.
#[allow(unsafe_code)]
pub fn is_hardware(f: &frame::Video) -> bool {
    // SAFETY: `as_ptr` is non-null for any `frame::Video` (even `empty()` allocates an AVFrame),
    // and `f` keeps it alive while we read one field.
    unsafe { !(*f.as_ptr()).hw_frames_ctx.is_null() }
}

/// Copies a GPU frame to system memory (usually NV12), keeping pts and colour tags.
#[allow(unsafe_code)]
pub fn download(f: &frame::Video) -> Result<frame::Video, Error> {
    // FFmpeg dereferences the source's hw context unchecked.
    if !is_hardware(f) {
        return Err(Error::InvalidData);
    }
    let mut out = frame::Video::empty();
    // SAFETY: both frames are valid and owned; `out` is empty, so FFmpeg allocates its buffers.
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
    use crate::test_media::corpus;

    /// Tests flip the global setting; serialise them.
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn context(file: &str) -> Option<codec::Context> {
        let dir = corpus()?;
        crate::ffmpeg::init().unwrap();
        let input = ffmpeg_next::format::input(&dir.join(file)).unwrap();
        let stream = input.streams().best(ffmpeg_next::media::Type::Video)?;
        codec::Context::from_parameters(stream.parameters()).ok()
    }

    #[test]
    fn disabled_or_other_codecs_stay_software() {
        let _g = LOCK.lock().unwrap();
        let Some(mut ctx) = context("prores.mov") else {
            return;
        };
        assert_eq!(attach(&mut ctx), None);
        let mut ctx = context("no-audio.mp4").unwrap();
        set_enabled(false);
        let got = attach(&mut ctx);
        set_enabled(true);
        assert_eq!(got, None);
    }

    #[test]
    fn software_frames_are_not_hardware() {
        let _g = LOCK.lock().unwrap();
        assert!(!is_hardware(&frame::Video::new(
            ffmpeg_next::format::Pixel::YUV420P,
            4,
            4
        )));
        // A software frame has no hw context to transfer from.
        assert!(
            download(&frame::Video::new(
                ffmpeg_next::format::Pixel::YUV420P,
                4,
                4
            ))
            .is_err()
        );
    }

    /// Each injected hardware failure (open, mid-stream decode, download) falls back to software
    /// and still returns the right frame.
    #[test]
    fn injected_hw_failures_fall_back_to_software() {
        let Some(dir) = corpus() else { return };
        crate::ffmpeg::init().unwrap();
        let _g = LOCK.lock().unwrap();
        let path = dir.join("frame-index-1080p30.mp4");
        let t = jvid_core::time::Time(jvid_core::time::FLICKS_PER_SECOND);
        let want = crate::decode::VideoDecoder::open(&path)
            .unwrap()
            .frame_at(t)
            .unwrap()
            .pts;
        for stage in [FAIL_OPEN, FAIL_DECODE, FAIL_DOWNLOAD] {
            FAKE_FAIL.store(stage, Ordering::Relaxed);
            let d = crate::decode::VideoDecoder::open(&path);
            let got = d.and_then(|mut d| {
                let f = d.frame_at(t)?;
                Ok((d.hwaccel(), f.pts))
            });
            FAKE_FAIL.store(0, Ordering::Relaxed);
            assert_eq!(got.unwrap(), (None, want), "stage {stage}");
        }
    }

    /// Without a GPU (CI) this covers the fallback; with one, the download path.
    #[test]
    fn hardware_or_fallback_decodes_the_same_frame() {
        let Some(dir) = corpus() else { return };
        crate::ffmpeg::init().unwrap();
        let _g = LOCK.lock().unwrap();
        for file in ["frame-index-1080p30.mp4", "4k-hevc.mp4"] {
            let path = dir.join(file);
            let t = jvid_core::time::Time(jvid_core::time::FLICKS_PER_SECOND);
            let mut hw = crate::decode::VideoDecoder::open(&path).unwrap();
            tracing::info!(file, hw = ?hw.hwaccel(), "decoding");
            let a = hw.frame_at(t).unwrap();
            set_enabled(false);
            let sw = crate::decode::VideoDecoder::open(&path);
            set_enabled(true);
            let mut sw = sw.unwrap();
            assert_eq!(sw.hwaccel(), None);
            let b = sw.frame_at(t).unwrap();
            assert_eq!(
                (a.pts, a.width, a.height),
                (b.pts, b.width, b.height),
                "{file}"
            );
            let diff: u64 = (a.planes[0].iter().zip(&b.planes[0]))
                .map(|(x, y)| x.abs_diff(*y) as u64)
                .sum();
            assert!(
                diff / a.planes[0].len() as u64 <= 2,
                "{file}: mean luma diff"
            );
        }
    }
}
