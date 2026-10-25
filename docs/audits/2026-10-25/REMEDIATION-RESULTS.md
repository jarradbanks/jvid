# Remediation results: issues #432–#474

All 43 primary implementations and integration follow-ups are merged locally on `audit/432-474-integration`. Final functional verification passed against product commit `f28cf2b2f4cf9a5287e98c4277053d39f5298c2e`. Performance limits are recorded below.

The work reused three Orca worktrees across dependent batches. Every issue has a separate primary commit and local `audit/issue-N` ref. The implementation phase ended with all changes local; subsequent PR publication was authorized separately and is recorded below. The original audit files were preserved.

## Issue coverage

| Issue | Primary commit | Implemented behavior |
| --- | --- | --- |
| [#432](https://github.com/jarradbanks/jvid/issues/432) | `8b82ce8` | Stage and validate lossless output; preserve prior destinations and reject source aliases. |
| [#433](https://github.com/jarradbanks/jvid/issues/433) | `2b68b29` | Finalize voiceover before Save, Close, New and Open; preserve unrelated undo history. |
| [#434](https://github.com/jarradbanks/jvid/issues/434) | `af51294` | Respect locks across the full linked ripple edit. |
| [#435](https://github.com/jarradbanks/jvid/issues/435) | `3260a02` | Apply ripple deletion once across linked audio/video timelines. |
| [#436](https://github.com/jarradbanks/jvid/issues/436) | `4175a8d` | Give separate gestures and Save boundaries separate undo steps. |
| [#437](https://github.com/jarradbanks/jvid/issues/437) | `3803dc4` | Reset editor state and reject abandoned-session jobs on project replacement. |
| [#438](https://github.com/jarradbanks/jvid/issues/438) | `6c5acba` | Keep Inspector edits attached to the active timeline selection. |
| [#439](https://github.com/jarradbanks/jvid/issues/439) | `c125d41` | Normalize ordinary/verbatim Windows roots and save portable relative media paths. |
| [#440](https://github.com/jarradbanks/jvid/issues/440) | `723cbe7` | Preserve delayed audio stream timestamps through playback, seeks and export. |
| [#441](https://github.com/jarradbanks/jvid/issues/441) | `f0f4d1f` | Compensate noise-reduction latency, preroll and audio tails. |
| [#442](https://github.com/jarradbanks/jvid/issues/442) | `a0c5877` | Use the explicit microphone stream consistently across speech and silence tools. |
| [#443](https://github.com/jarradbanks/jvid/issues/443) | `aa350f9` | Version transcript language and use language-specific filler lists. |
| [#444](https://github.com/jarradbanks/jvid/issues/444) | `cee4bbb` | Map nested subtitle cues and safely publish media/SRT pairs while preserving titles. |
| [#445](https://github.com/jarradbanks/jvid/issues/445) | `14de36a` | Export PNG from the full Program composition at the timeline playhead. |
| [#446](https://github.com/jarradbanks/jvid/issues/446) | `7dac83d` | Preserve imported still alpha and refresh cached source versions away from repaint. |
| [#447](https://github.com/jarradbanks/jvid/issues/447) | `bc3a97b` | Use explicit BT.709 limited-range SDR conversion and encoder/frame tags. |
| [#448](https://github.com/jarradbanks/jvid/issues/448) | `622146b` | Apply metadata-aware color conversion to thumbnails and proxies. |
| [#449](https://github.com/jarradbanks/jvid/issues/449) | `fed3c34` | Keep pixel-sized effects stable across preview quality and proxy resolutions. |
| [#450](https://github.com/jarradbanks/jvid/issues/450) | `ee1b40c` | Keep encoder fallback in the requested codec family and validate the produced stream. |
| [#451](https://github.com/jarradbanks/jvid/issues/451) | `777104e` | Avoid GPU acquisition for audio-only, Copy and aligned Smart CLI exports. |
| [#452](https://github.com/jarradbanks/jvid/issues/452) | `d273cc7` | Give open modals ownership of keyboard input from their first frame. |
| [#453](https://github.com/jarradbanks/jvid/issues/453) | `c32494d` | Dispatch focused transport activation and global shortcuts exactly once. |
| [#454](https://github.com/jarradbanks/jvid/issues/454) | `5888e96` | Budget transport width and retain usable overflow/recording controls. |
| [#455](https://github.com/jarradbanks/jvid/issues/455) | `fc9877d` | Keep narrow Inspector sliders clear of adjacent Reset controls. |
| [#456](https://github.com/jarradbanks/jvid/issues/456) | `d863576` | Keep scope traces visible in light and high-contrast themes. |
| [#457](https://github.com/jarradbanks/jvid/issues/457) | `e19efeb` | Focus Source immediately after opening a bin asset. |
| [#458](https://github.com/jarradbanks/jvid/issues/458) | `e5c1dc9` | Move a multi-selection and linked groups with one common bounded offset. |
| [#459](https://github.com/jarradbanks/jvid/issues/459) | `ba60ada` | Keep marquee selection in model coordinates while scrolling. |
| [#460](https://github.com/jarradbanks/jvid/issues/460) | `5231a66` | Paste keys into the selected supported destination with valid parameter ownership. |
| [#461](https://github.com/jarradbanks/jvid/issues/461) | `927c968` | Clear explicitly discarded recovery versions without changing other projects. |
| [#462](https://github.com/jarradbanks/jvid/issues/462) | `0baddd5` | Keep Match source and Added order deterministic despite out-of-order probes. |
| [#463](https://github.com/jarradbanks/jvid/issues/463) | `4c98008` | Add frame-clamped Source/Program seek strips with audio flush and coalesced decode. |
| [#464](https://github.com/jarradbanks/jvid/issues/464) | `2cfa621` | Fan out filmstrip results, refresh source generations off-thread and stop failed-hover retry loops. |
| [#465](https://github.com/jarradbanks/jvid/issues/465) | `b62134d` | Cache source-level silence scans and map them through clip timing/generations. |
| [#466](https://github.com/jarradbanks/jvid/issues/466) | `ba4a94e` | Wake Program after current decode success/failure without continuous idle repaint. |
| [#467](https://github.com/jarradbanks/jvid/issues/467) | `b9e7620` | Cancel obsolete stand-ins before decoding and reject stale results. |
| [#468](https://github.com/jarradbanks/jvid/issues/468) | `15426c1` | Cancel waveform work between chunks and before publishing caches. |
| [#469](https://github.com/jarradbanks/jvid/issues/469) | `d969c15` | Honor the autosave interval and zero/off setting. |
| [#470](https://github.com/jarradbanks/jvid/issues/470) | `27d9702` | Implement Focus Effects through existing bin/dock focus routing. |
| [#471](https://github.com/jarradbanks/jvid/issues/471) | `044056e` | Read live key bindings for translated transport hints and accessibility labels. |
| [#472](https://github.com/jarradbanks/jvid/issues/472) | `8e7b759` | Fit shortcut-editor chrome and the action list to reduced logical viewports. |
| [#473](https://github.com/jarradbanks/jvid/issues/473) | `02625c4` | Report audio analysis, encoding passes, retries and finalization in UI and CLI JSON. |
| [#474](https://github.com/jarradbanks/jvid/issues/474) | `40fc54c` | Reuse upload planes, uniforms and leased output textures with bounded ownership. |

The final tree also includes the focused review follow-ups: Windows symlink privilege handling (`439ab07`), decoder cleanup before Windows fixture removal (`d080b6e`), waveform generation handoff (`252612b`), nested-container render preservation (`cee4bbb`), filmstrip paint-path metadata removal (`2cfa621`), translated effect descriptions/thumbnails (`432d782`), seek-strip geometry fixtures (`8c7a3df`, `a65b274`), failed-hover retry/terminal classification (`f3e2e1f`), codec-independent settings geometry fixtures (`e9d1e0f`), and modal ownership during shortcut batches (`f69a70c`). Primary refs for #444, #464 and #465 include their accepted corrections; additional integration follow-ups are preserved in history.

## Validation

| Check | Source | Result |
| --- | --- | --- |
| Formatting | `f28cf2b` | Passed. |
| Strict workspace Clippy, all targets | `f28cf2b`, default and GPL | Passed with `-D warnings`. |
| Default full workspace | `f28cf2b` | **972 passed, 0 failed, 9 ignored**. |
| GPL full workspace | `f28cf2b` | **974 passed, 0 failed, 9 ignored**. |
| Locale-mutating UI cases | `6f20bf6` | All three run individually and passed. Affected UI implementations are unchanged in the final source. |
| Native Windows final media/preview/CLI selection | `6f20bf6` | **95 reported passes**, no failures or ignores; four debug playback timing cases return early and are not performance evidence. |
| Native Windows final modal/input follow-up | `f28cf2b` | **36 passed**, no failures or ignores. |
| Optimized audio/media gates | `9c592fe`, unchanged components at `f28cf2b` | All three passed. |
| Optimized app comparison | `6f20bf6` versus original `7efe671` | Mixed; see the qualified results below. |

The final Linux commands were `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, and `cargo test --workspace --no-fail-fast -- --test-threads=2`. GPL checks add `--features jvid-app/gpl-ffmpeg,jvid-media/gpl-ffmpeg`. Media, GPU and x265 requirements were enabled, using the complete generated corpus. All 31 test binaries, including doctests, completed for each workspace run.

The nine ordinary ignores comprise three separately verified locale cases, the separately verified optimized 500-clip and 16-track cases, the application reel, a long scene-highlights timing case, and two downloaded speech-model cases. The latter four were not run. Debug early returns are not counted as actual timing or model-inference verification.

Completed optimized media/audio checks used the all-43 source at `9c592fe` before app-only review follow-ups. All three selected tests passed: 16-track mixing used 0.042% of real time, one-hour waveform queries took 9.6–29.2 μs per 2,000 pixels, and two minute-long lossless copy checks took 110–165 ms. A Git comparison confirms the core/media/audio sources, Cargo manifests/lockfile and Cargo configuration are unchanged at the final product commit.

The #474 upload regression measured 30 real 1080p uploads after warm-up. The original implementation created 90 plane textures, 30 conversion buffers, 30 converted images and 60 bind groups. The corrected implementation created zero of each in the same measured interval. Normal Source paint and Program composition have separate reuse checks. Retained leases beyond pool capacity, delayed command submission, scope captures, metadata changes, odd dimensions, Full/Half/Quarter, SDR/PQ/HLG and linear/sRGB targets are covered.

Earlier native Windows runs passed the real import/save/move/reload path, voiceover lifecycle, direct/hard-link alias safety, audio timing/latency, edit/undo/layout behavior, full Program PNG/still-alpha/effects, codec-family/timing cases, nested sidecars, CPU-only CLI exports, and actual CLI/app byte parity. The final 95-test native selection checked the later filmstrip, seek, phase and GPU-lease changes; the subsequent 36-test run checked the final modal/input change. Native selections are targeted and are not presented as a whole Windows workspace pass.

Linux checks use Rust 1.99.1, FFmpeg 4.4.2 and Vulkan lavapipe with a complete generated corpus and required media/GPU flags. Native checks use Rust 1.99.0, FFmpeg 8.1 and required GPU/corpus checks; Windows OpenH264 is usable. Actual H.264 proxy/content cases on Linux run with explicit `gpl-ffmpeg` rather than a different codec fallback.

Optimized app measurements used frozen `6f20bf6`; the later product change only affects modal shortcut batches, with a separate settings test-fixture adjustment. In a compiler-free six-case comparison, both versions passed four cases and failed two. Corrected scrubbing delivered 46.3 exact updates/s against the 30/s budget (original 1.8/s); 1080p seek median was 4.2 ms against 33 ms, and 500-clip layout/paint was 1.427 ms against 16.7 ms. The corrected long-recording test initially exceeded its 150 ms worst-first-frame budget at 446 ms. A separate uninstrumented confirmation passed at 73.8 ms, while the original exceeded it at 156 ms, showing timing variance on this host. The two-layer frame-perfect playback case dropped frames on **both** versions. Informational cold-start/RSS measurements also exceeded their budgets on both. Budgets were kept unchanged; these results do not establish physical GPU throughput or an all-green performance suite.

## Review findings resolved during integration

- A shortcut opening a modal originally allowed later actions from the same input batch and focused background controls to activate; ownership is now rechecked after each action and background focus is surrendered. Eight negative regressions failed before the fix; all 20 B11 tests now pass.
- Recording finalization originally absorbed unrelated edits; it now uses short, separate undo transactions.
- Nesting a caption and ordinary title onto a subtitle-marked container initially hid both; only subtitle text is suppressed in the temporary render copy.
- The first filmstrip fanout performed per-clip filesystem reads on paint; generation checks now use shared memory and bounded worker refreshes.
- Slow failed-hover completion could sustain its own repaint/retry loop; completion now drains to idle before retry rearming.
- A pooled upload initially used its sRGB sampling view as a linear conversion target; separate views and new SDR/PQ/HLG regressions corrected it.
- Available encoder capabilities changed the HDR checkbox brightness in a settings geometry snapshot; that fixture now pins its own HDR state, while separate tests verify actual HDR availability.
- The combined UI run exposed intentional seek-strip geometry and alpha-sampling thumbnail changes plus translated shader description drift; focused fixtures/descriptions were corrected without weakening snapshot tolerances or translation guards.

## Verification limits and retained contracts

- Windows symbolic links could not be exercised where privilege error 1314 prevented their creation. Direct paths/hard links were checked natively; Unix symbolic aliases were checked on Linux. Actual network UNC shares were unavailable.
- Two native FFmpeg 8.1 media tests already fail on a clean original `7efe671` rebuild: `decode::tests::corrupt_packets_are_skipped` and `encode::tests::mp3_lasts_its_samples`. They are not new remediation failures; final native validation is targeted and does not claim a whole Windows workspace pass.
- The optimized two-layer playback throughput gate remains failing on this host on both original and corrected code. The long-recording gate varies between runs; the final uninstrumented corrected confirmation passed.
- Physical microphone/speaker behavior, real GPU throughput and downloaded speech-model inference are not claimed from fake-device tests or lavapipe. Locale-mutating tests run separately. Debug timing returns and ordinary ignored tests are not performance evidence.
- GPU `Image` leases protect converted upload frames. The existing final Compositor output remains borrowed/reused across draws; immutable final snapshots require a copy. Current scope/thumbnail/export reads submit before later writes.
- Media/SRT publication restores prior outputs on tested publication failure and retains recovery files if restoration fails. This is rollback-protected publication, not a crash-atomic two-file transaction.
- Completed externally modified filmstrips refresh at explicit request, hover re-entry or focus-return boundaries. Source identity follows size/mtime. A single in-progress native decoder call cannot be preempted; cancellation is checked at its next bounded boundary.
- Masked pixel effects preserve the legacy Full-quality enlargement reference, including its existing difference from unmasked source-relative sizing. Preview/proxy scaling stays consistent with that saved-project behavior.

## PR publication and CI follow-ups

[PR #475](https://github.com/jarradbanks/jvid/pull/475) integrates this branch into `main`, with a merge commit requested to retain individual issue history. GitHub is authoritative for its live CI and merge state.

The initial hosted [CI run](https://github.com/jarradbanks/jvid/actions/runs/40192553366) exposed two validation portability gaps. Runtime audio fixtures invoke the `ffmpeg` executable, but the workflow installed it only when generating an uncached corpus. The workflow now installs it alongside the development libraries on every Linux run (`e1ad18a`).

The ProRes check also assumed that a decoder context always contains its limited-range flag. MOV's `nclc` color atom has no range field, and FFmpeg 6.1 sets ProRes range on decoded frames; the [MOV muxer](https://github.com/FFmpeg/FFmpeg/blob/n6.1/libavformat/movenc.c#L1990) and [ProRes decoder](https://github.com/FFmpeg/FFmpeg/blob/n6.1/libavcodec/proresdec2.c#L267) show those contracts. The test now strictly checks matrix/range/primaries/transfer on actual decoded frames for every exercised encoder, retains stream color tags and the MP4 range check, and keeps the same pixel tolerances (`3e8c1a3`). No runtime encoding behavior changed.

The follow-up passed formatting, strict all-target workspace Clippy, and both the color and independent-offset audio regressions on Linux FFmpeg 4.4 and native Windows FFmpeg 8.1. Hosted CI then verifies the corrected tests and cache-hit fixture path on FFmpeg 6.1. Raw evidence is in `ci-followup-{fmt,clippy,media}.log`, `ci-followup-results.json` and `native-ci-followup.log` in the session evidence directory.

## Review artifacts

The machine-readable issue/commit/status manifest is [remediation-plan.json](remediation-plan.json); the batch/dependency plan is [REMEDIATION-PLAN.md](REMEDIATION-PLAN.md). Detailed per-batch reports, frozen-source manifests and raw logs are retained under `/tmp/jvid-remediation-2026-10-25/` in this session. Key evidence: `final-validation-summary.json`, `final-combined-results.json`, `final-combined-workspace-default.log`, `final-combined-workspace-gpl.log`, `final-locale-*.log`, `native-final.log`, `native-modal.log`, `release-final-cpu.log`, `release-quiet-comparison.json`, `release-quiet-{final,original}.log`, `release-long-confirm.json`, `release-long-confirm-{final,original}.log`, and `final-issue-ref-check.json`. All 43 issue refs were checked as ancestors of the tested product commit. These temporary logs are session evidence, not repository prerequisites.

Work was coordinated through the local [Orca CLI skill](/home/jarrad/.agents/skills/orca-cli/SKILL.md) and [orchestration skill](/home/jarrad/.agents/skills/orchestration/SKILL.md). Settled worker terminals were released; the final lane-C terminal was retained by Orca with reason `user_takeover` and left untouched.
