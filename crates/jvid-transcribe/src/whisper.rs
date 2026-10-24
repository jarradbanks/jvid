//! [`Transcriber`] on whisper.cpp, CPU only.

use std::path::Path;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::Duration;

use jvid_core::time::Time;
use jvid_jobs::Ctx;
use whisper_rs::{
    FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters, WhisperError,
    WhisperState, WhisperTokenId,
};

use crate::{Error, Options, Transcriber, Transcript, Word};

/// Whisper reports times in centiseconds.
const FLICKS_PER_CS: i64 = jvid_core::time::FLICKS_PER_SECOND / 100;

/// A loaded whisper.cpp model; one can serve several jobs. On Cancel a job returns within a second,
/// but whisper.cpp only checks for it between compute graphs, so the CPU and the job's working
/// memory are freed at the next graph boundary: up to one encoder pass later.
///
/// Words are split on spaces, so languages written without them (Chinese, Japanese) come back as
/// sentence-long "words".
pub struct Whisper(WhisperContext);

impl Whisper {
    /// Loads a ggml model file such as one from [`crate::models::Model::path`]. It reads the whole
    /// file and can take seconds, so load it on the job pool, not the UI thread.
    pub fn load(model: &Path) -> Result<Self, Error> {
        whisper_rs::install_logging_hooks();
        WhisperContext::new_with_params(model, WhisperContextParameters::default())
            .map(Self)
            .map_err(|e| Error::Failed(format!("can't load {}: {e}", model.display())))
    }
}

impl Transcriber for Whisper {
    fn transcribe(&self, audio: Vec<f32>, opts: &Options, ctx: &Ctx) -> Result<Transcript, Error> {
        let state = self.0.create_state().map_err(failed)?;
        let eot = self.0.token_eot();
        let (opts, job) = (opts.clone(), ctx.clone());
        let (tx, rx) = mpsc::channel();
        // whisper.cpp only checks for abort between compute graphs, and one encoder pass of a large
        // model can take seconds on a CPU: Cancel returns now and the thread stops at that check.
        std::thread::Builder::new()
            .name("jvid-whisper".into())
            .spawn(move || tx.send(full(state, eot, &audio, &opts, &job)))
            .map_err(|e| Error::Failed(e.to_string()))?;
        loop {
            match rx.recv_timeout(Duration::from_millis(20)) {
                Ok(r) => return r,
                Err(RecvTimeoutError::Timeout) if ctx.is_cancelled() => {
                    return Err(Error::Cancelled);
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(Error::Failed("whisper stopped unexpectedly".into()));
                }
            }
        }
    }
}

fn failed(e: WhisperError) -> Error {
    Error::Failed(e.to_string())
}

fn full(
    mut state: WhisperState,
    eot: WhisperTokenId,
    audio: &[f32],
    opts: &Options,
    ctx: &Ctx,
) -> Result<Transcript, Error> {
    let mut p = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
    p.set_language(opts.language.as_deref());
    if let Some(prompt) = &opts.initial_prompt {
        p.set_initial_prompt(prompt);
    }
    // One word per segment: whisper.cpp's own word-level output (`-ml 1`).
    p.set_token_timestamps(true);
    p.set_max_len(1);
    p.set_split_on_word(true);
    p.set_print_progress(false);
    p.set_print_realtime(false);
    p.set_print_timestamps(false);
    // No "[BLANK_AUDIO]" or "(music)" as words; fillers like "um" are speech and stay.
    p.set_suppress_nst(true);
    // whisper-rs 0.16 leaks these boxed callbacks (each holding a Ctx clone) and the language and
    // prompt strings on every call: a few bytes per transcription.
    let c = ctx.clone();
    p.set_progress_callback_safe(move |pct: i32| c.set_progress(pct as f32 / 100.0));
    let c = ctx.clone();
    // whisper.cpp checks this only after each whole encoder/decoder graph: its sched-based
    // `ggml_graph_compute_helper` (src/whisper.cpp) never gives it to the CPU backend, which would
    // check per node. Nothing to change here once upstream fixes that.
    // F spelled out: whisper-rs 0.16 calls the abort closure through `&mut F` but stores a
    // `Box<dyn FnMut>`, which only agree when F is that box.
    p.set_abort_callback_safe::<_, Box<dyn FnMut() -> bool>>(
        Box::new(move || c.is_cancelled()) as Box<dyn FnMut() -> bool>
    );
    let r = state.full(p, audio);
    if ctx.is_cancelled() {
        return Err(Error::Cancelled);
    }
    r.map_err(failed)?;

    let mut words = Vec::new();
    for s in state.as_iter() {
        let text = s.to_str_lossy().map_err(failed)?.trim().to_owned();
        // Text tokens only: special ones ([_BEG_], timestamps) sit at or above end-of-text.
        let p: Vec<f32> = (0..s.n_tokens())
            .filter_map(|i| s.get_token(i))
            .filter(|t| t.token_id() < eot)
            .map(|t| t.token_probability())
            .collect();
        if text.is_empty() || p.is_empty() {
            continue;
        }
        words.push(Word {
            text,
            start: Time(s.start_timestamp() * FLICKS_PER_CS),
            end: Time(s.end_timestamp() * FLICKS_PER_CS),
            confidence: p.iter().sum::<f32>() / p.len() as f32,
        });
    }
    let language = whisper_rs::get_lang_str(state.full_lang_id_from_state()).map(str::to_owned);
    Ok(Transcript { language, words })
}
