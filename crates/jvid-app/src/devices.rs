//! Developer display sessions use the existing wall clock without opening audio hardware.

use jvid_audio::{Backend, CpalBackend, CpalInput, InputBackend};

fn isolated() -> bool {
    #[cfg(all(debug_assertions, feature = "debug-ui"))]
    return std::env::var_os("JUI_DEBUG").is_some_and(|value| value == "1");
    #[cfg(not(all(debug_assertions, feature = "debug-ui")))]
    false
}

pub(crate) fn output() -> Box<dyn Backend> {
    if isolated() {
        return Box::new(NoDevices);
    }
    Box::new(CpalBackend::default())
}

pub(crate) fn input() -> Box<dyn InputBackend> {
    if isolated() {
        return Box::new(NoDevices);
    }
    Box::new(CpalInput::default())
}

struct NoDevices;

impl Backend for NoDevices {
    fn devices(&self) -> Vec<String> {
        Vec::new()
    }

    fn open(
        &mut self,
        _: Option<&str>,
        _: std::sync::Arc<jvid_audio::Feed>,
    ) -> Result<(String, Box<dyn std::any::Any>), String> {
        Err(crate::i18n::tr!("debug.audio_disabled").to_owned())
    }
}

impl InputBackend for NoDevices {
    fn devices(&self) -> Vec<String> {
        Vec::new()
    }

    fn format(&self, _: Option<&str>) -> Result<jvid_audio::Format, String> {
        Err(crate::i18n::tr!("debug.audio_disabled").to_owned())
    }

    fn open(
        &mut self,
        _: Option<&str>,
        _: std::sync::Arc<jvid_audio::Capture>,
    ) -> Result<Box<dyn std::any::Any>, String> {
        Err(crate::i18n::tr!("debug.audio_disabled").to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn isolated_output_uses_the_existing_fallback_clock_and_input_cannot_record() {
        let mut out = jvid_audio::Output::new(Box::new(NoDevices), None);
        assert_eq!(out.device(), None);
        assert!(out.devices().is_empty());
        assert_eq!(out.push(&[0.0; 960]), 960);
        assert!(InputBackend::devices(&NoDevices).is_empty());
        assert!(InputBackend::format(&NoDevices, None).is_err());
    }
}
