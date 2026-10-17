# Performance budgets

The one place the budgets live. Targets come from [PLAN.md · Performance targets](PLAN.md#performance-targets).
CI reports every number in the PR's job summary (the `perf` steps of the Linux `check` job). It
does not fail on them: hosted runners are noisy and render on lavapipe, a software rasterizer.
Hard gates that already existed stay as plain tests and still fail CI (marked *gated*).

| Measure | Budget | Where | Gate |
|---|---|---|---|
| Cold start, app construction to first frame | 1000 ms | `jvid-app` `app::tests::perf_cold_start_and_idle_memory` | report |
| Idle memory, RSS after 60 idle frames | 150 MB | same test (Linux only) | report |
| Seek latency, parked seek on 1080p30 H.264 | 33 ms (one frame) | `jvid-app` `playback::tests::seek_latency_1080p30` | report |
| Long recording, 2 h 1440p60: import to editable | 2000 ms | `jvid-app` `playback::tests::long_recording_imports_and_seeks_fast` (#90) | report |
| Long recording, seek anywhere to a frame on screen (keyframe stand-in) | 150 ms | same test | report |
| Playback, two 1080p30 layers + audio, decode | 30 fps, zero drops | `jvid-app` `playback::tests::two_layer_1080p30_plays_in_real_time` (#32) | gated |
| Composite, same timeline at preview scale | 33 ms/frame on a real GPU | same test | report |
| Timeline layout + paint, 500 clips | 16.6 ms | `jvid-app` `timeline::tests::five_hundred_clips_fit_a_frame` (#33) | gated |
| Waveform query, 1 h at any zoom | 1 ms | `jvid-media` `waveforms::tests::one_hour_query_under_1ms_at_any_zoom` (#22) | gated |
| Mixer, 16 tracks | 5% of real time | `jvid-audio` `mixer::tests::sixteen_tracks_under_5_percent_of_real_time` (#31) | gated |
| Timeline evaluation, 8 tracks × 500 clips | report | `cargo bench -p jvid-core` `video_at` | report |
| Keyframe interpolation, 1000 keys | report | `cargo bench -p jvid-core` `value_at` | report |

The release tests print a `PERF …` line each. Run them all locally (needs `scripts/gen-test-media`):

```sh
cargo test --release -p jvid-app --bin jvid-app -- perf_cold_start --nocapture   # alone, so RSS is clean
cargo test --release -p jvid-app --bin jvid-app -- seek_latency long_recording two_layer five_hundred_clips --nocapture
cargo test --release -p jvid-media -p jvid-audio -- one_hour_query sixteen_tracks --nocapture
cargo bench -p jvid-core --bench core
```

Cold start and memory are measured in-process, so they leave out exec and dynamic linking and
include the test harness: a ceiling on the real numbers' trend, not the user-visible value.
