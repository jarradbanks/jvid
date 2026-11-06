# jvid workflow improvements — execution plan

Approved by the owner on 2026-11-06: proposals 1, 2, 3, 4, 5, 6, 7, 9, 10, 12, 13, 14 and 15, plus comfortable Fit framing and reachable transform controls in Program and Source/clip preview. Implement these scoped first versions, validate them, and merge to main. Proposals 8 (audio roles) and 11 (history/checkpoints) are excluded.

## Execution ownership

A new Codex orchestrator using `gpt-6.1-sol`, effort `xhigh`, owns execution, Orca worker coordination on Omarchy, integration, review, and merging to main. This planning agent hands over ownership rather than supervising a second coordinator. Use the installed orca-cli and orchestration skills and their version-matched references. Do not substitute internal subagents for Orca workers.

Local project: `github:jarradbanks/jvid`; repository ID `1c41a0e9-2a15-4d45-82e5-3d74218a2274`; main checkout `C:\Users\Jarrad\orca\projects\jvid` (WSL `/mnt/c/Users/Jarrad/orca/projects/jvid`). Initial observed main HEAD: `e49d81fa86204e6278584e96405194d3f2ba4dc1`.

Omarchy environment ID: `03931d87-a4af-4449-8d6a-3151dbfaf56c`, name `omarchy`. Discover its exact repository and capabilities before remote placement. Use `worker-start --on omarchy` from the coordinator and follow-ups by Dispatch ID. No silent local substitution if Omarchy is unavailable. Remote contact loss is unverifiable, not process death.

The local repository default worktree base is `origin/develop`, but this user explicitly wants integration into **main**. Explicitly align the integration branch and worker bases with current main; do not accidentally use develop. Fetch and inspect divergence before integration. Existing `.github/workflows/ci.yml` changes and untracked audit/testing files in the user's main checkout are unrelated: do not stage, revert, stash or overwrite them. Use an isolated orchestrator checkout and isolated worker branches where shared-file conflicts require it.

## Common acceptance and boundaries

- Preserve the quick-editor product, DESIGN.md conventions, remappable shortcuts, localization, narrow-panel layouts, and Windows/macOS/Linux behavior. Reconcile against actual code rather than stale roadmap claims.
- Timeline/content edits must be undoable, serialize correctly, and agree in playback/export. View settings must not mutate project media or rendering. Maintain old project/config compatibility.
- Worker task specs must name exact owned modules, shared-file integration boundaries, dependencies, observable acceptance, and tests. New modules should hold substantial logic; coordinate app.rs, history.rs, model.rs, keymap, catalogs and docs patches centrally.
- Introduce shared contracts in small commits before consumers. No concurrent edits of one checkout. Cap the initial wave around four Omarchy workers and adjust to actual CPU/RAM/build capacity; do not launch every task simultaneously.
- Each worker reports commits, changed files, test commands/results, UI evidence where appropriate, and limitations. Independently review returned changes before accepting them. Add meaningful regression tests for editing, persistence and coordinate behavior, not tests mirroring trivial UI code.
- Run required fmt/lint/tests and integration checks on the final merged tree. Cover representative 720p and 1080p windows, 100% and 150% scale, long translated strings, keyboard focus, cancel/undo/reopen, and affected preview/export paths. Run platform CI; clearly report any physical-device/manual checks that remain.
- Merge through the repository's established protected-main workflow after required checks pass. Resolve conflicts and integrate all approved scope; do not stop at draft PRs. Never bypass branch protection or force-push main. No release/tag/publishing task is authorized here.

## Work packages

### V — Viewer Fit and transform access (new owner request)

Target: preview.rs, handles.rs, shared viewer geometry, applicable Source editing, zoom/mask/text overlays.

Fit currently risks placing edge and rotation controls outside the clipped viewer. Reserve a comfortable screen-space editing gutter so a normally fitted clip has reachable corner/edge/rotation controls. Keep the underlying frame mapping consistent for rendering, hit testing, text, masks, zoom-to-region and reframing. Do not shrink the actual clip or change exported framing. Handle oversize/rotated/off-frame selections with a deliberate recovery affordance (e.g. view selection), without unstable automatic zoom during a drag. Apply consistent view behavior in Program and Source; preserve Source's existing edit capabilities rather than inventing source edits. Keep overlays and hit targets out of transport controls and neighboring panels.

Acceptance: fitted landscape/portrait/square clips and editable titles have reachable controls; rotations and large transforms can be recovered; narrow and high-DPI viewers work; zoom/pan, masks and Alt-region gestures retain correct coordinates; project transform values and rendered output do not change merely by fitting the view. Include geometry tests and before/after UI evidence.

### A1 — Command palette (#1)

Target: app action dispatch, keymap, new palette module. Search existing registered actions by translated name and useful aliases; show current shortcuts and availability reasons; support keyboard navigation, Enter and Escape. Reuse existing action execution and applicability checks. Text fields must retain typing shortcuts. New feature actions register through the same contract. Acceptance includes custom/unbound shortcuts, focus restoration, disabled actions and language switches.

### A2 — Workspaces and maximize (#2)

Target: dock layout/settings persistence. Ship Editing, Captions and Audio layouts, save/rename/delete personal layouts, and reversible maximize of the focused panel. Existing panel state, jobs and transcript cache survive layout changes. Preserve persisted layouts and repair invalid old layouts safely. Establish a stable optional-panel ID contract for caption/highlight workspaces before their consumers land. Test restart, maximize/restore, reset and missing optional panels.

### H — Highlight review (#3)

Target: marker/highlight domain, new review panel, export handoff. List manual and detected highlights with thumbnail, name, duration, audition and editable boundaries; select subsets for export. Reuse ranged markers and established point-marker padding rather than introducing a conflicting range store. Honor linked tracks and undo for marker edits; detect stale selections after timeline changes. Test subset export, range edits, point/range markers, overlaps and long-source responsiveness.

### C — Caption workspace (#4)

Target: subtitles/transcript integration, new caption panel and core caption commands. Editable rows with text/start/end, split/merge, search/replace and shared caption styling. Existing caption clips remain authoritative. Shared style applies to the chosen caption scope without changing unrelated titles; define explicit per-caption override behavior. Integrate as an optional panel through A2. Test Unicode, timing boundaries/overlaps, hidden tracks, undo/reopen, SRT and burned-in output. Animated word highlighting and translation are later scope.

### P1 — Selective settings transfer (#10)

Target: core commands/settings extraction, inspector integration. Copy/paste selected transform, grade, effect stack, audio or text categories. Distinguish static settings from animation; make replace semantics explicit and validate clip-kind compatibility. Default to static settings; animation is opt-in with defined relative-time mapping and handling of keys beyond destination duration. One undoable operation across target clips. Preserve links/source identity/duration. Test mismatched clip types, durations, missing effects and save/reopen.

### P2 — Personal presets (#5)

Depends on P1's reusable transfer/validation contract. Ship saved title styles, voice treatments and visual effect stacks, delivered category by category. Named save/apply/rename/delete, persistent versioned user storage and useful previews/labels. Missing fonts/effects produce actionable feedback while preserving the saved definition. Titles carry style, not words or source identity; voice treatments do not silently replace timing/links. Reuse P1 semantics for animation. Test cross-project use, restart, invalid files and compatibility. Full project templates are excluded.

### S — Review automatic speech cuts (#6)

Target: silence detection, transcript/fillers, new proposal review UI. Audition before/after cuts, keep/reject individual suggestions, tune pause threshold and retained breathing room, show time saved, then apply one undoable operation. Analyze without immediately mutating the timeline. Record source/project revision and invalidate or recompute proposals after conflicting edits. Preserve linked A/V sync, captions and locked-track rules. Test cancellation, stale analysis, all/none accepted, edge cuts, undo and export audio continuity.

### N — Screen annotations (#7)

Depends on V geometry contract; coordinate with P1/P2 style representation. Add manually placed arrows, outline rectangles and numbered callouts in Program with editable style, duration and simple entrance/exit animation. Use ordinary editable clips/keyframes and the common compositor so export matches preview. Scope a minimal vector/shape representation if required, with backward-compatible serialization. Test rotated/scaled viewers, ordering, undo/reopen and rendered output. Automatic tracking is excluded.

### T — Timeline feedback (#9)

Target: timeline drag/trim/snap feedback. Temporary overwrite/ripple preview, snap-target label and trim delta near the pointer. Feedback derives from the same planned edit as commit so it never promises a different result. Respect locked tracks and linked media; modifier changes update the preview. No decorative persistent clutter. Test insert/overwrite/ripple, snapping, autoscroll, cancellation and high-DPI/narrow layouts. Coordinate shared timeline integration with H and S.

### K — Collect project/media (#12)

Target: project file and path/media resolution, background copy job, new collection UI. Copy project plus all referenced original media into a new chosen directory; include nested sequences and external dependencies needed for faithful reopening (effects/LUTs/generated voiceover and fonts where bundling is permitted). Produce a dependency manifest and explicit unresolved-dependency report. Whole media files only; no trimming/transcoding. Deduplicate safely, handle same-name files, use portable relative paths, and publish the collected project only after successful verified copies. Cancellation/failure must leave originals and existing destinations intact. Acceptance: open the collected project with originals unavailable, with tests for nested refs, collisions, missing media and interrupted copying.

### E — Multi-version export (#13)

Depends on H's subset/range handoff and review of existing reframe behavior. One setup queues separately reviewed landscape, vertical and size-limited outputs with per-version preview, preset, naming and caption policy. Reuse existing queue and target-size functionality. Capture immutable per-version project/range settings so changing one variant never changes another or the user's edit. Require deliberate framing review for aspect changes. Test filename collisions, independent cancellation/failure, simultaneous jobs and actual output dimensions/ranges. Synchronizing independently edited aspect-ratio timelines is excluded.

### F — Playback status (#14)

Target: playback/proxy observability and a compact viewer status control. Show actual proxy generation/use, preview quality and measured playback trouble with an actionable explanation. Avoid fabricated performance claims and noisy always-on warnings. Status must not block decoding or request extra expensive work per frame. Coordinate viewer insertion after V; logic can proceed independently. Test idle/loading/failed/ready proxies, recovery, audio-only clips and status transitions.

### B — Media library (#15)

Target: media bin and backward-compatible project metadata. Favorites, simple bins and filters for Used/Unused, Video/Audio and Missing, combined with existing search/sort. Usage includes nested sequences. Moving a bin item does not move source files; deletion rules distinguish organization from removing an asset. Keep a flat default for quick edits. Test rename/move/delete, undo where project metadata changes, reopen, nested usage, missing media and large-bin responsiveness.

## Parallel waves and integration order

Wave 0: orchestrator validates baseline and remote capabilities, defines panel/action registration, viewer-coordinate and settings-transfer contracts, and records task DAG/ownership. It may delegate bounded discovery, but must start implementation once contracts are sufficient.

Initial parallel lanes (isolated Omarchy worktrees):

1. Viewer lane: V, then F and N (F and N can split after V if shared integration is serialized).
2. Shell lane: A1 then A2; expose registration contracts early so panel workers can start logic independently.
3. Settings lane: P1 then P2; coordinate N representation through the orchestrator.
4. Project lane: K and B as separate sequential tasks initially; they can split if model/path boundaries and machine capacity allow.

As capacity frees, start H, C, S and T in parallel on their own modules after the relevant contracts land. H/C require A2 only for final panel registration, not all domain work. S and T have no intrinsic dependency on H/C, but their timeline/core patches must be integrated serially. E follows H's export-selection contract and finalized variant framing decisions. Avoid artificial full-wave barriers: launch ready tasks when a slot is available.

The orchestrator owns shared app/menu/keymap wiring, schema/history integration conflicts, final catalogs/docs, and sequential merges. Workers may supply small patches in shared files but must declare them, and the orchestrator rebases/reviews against the latest integrated main before merging. Persistent project metadata changes in B/N/C must be coordinated, not independently version-bumped.

## Completion report

Report each package's outcome with merged commit/PR, meaningful tests and any limits. All fourteen selected packages must be accounted for. Distinguish implementation from manual/platform verification. Settle/reuse/retain/release every Orca worker per the skill; do not leave orphan worker ownership. The final user response should identify the integrated main commit and any remaining real blockers rather than merely reporting dispatched work.
