# Desktop debug UI

`jvid-app` has two explicit developer features. `hot-reload` connects Dioxus
Devtools and calls the editor frame through Subsecond 0.7. `debug-ui` installs
the shared `j-ui-debug` egui plugin when `JUI_DEBUG=1` and
`JUI_DEBUG_CONNECTION` names a unique absolute connection file. `JUI_DEBUG_PROFILE`
must name the disposable profile's absolute directory; config, data, cache,
recovery, themes, logs, models and window persistence stay below it on every OS.
Missing, invalid or unwritable opted-in profiles fail startup without a fallback
to installed folders. A portable package marker rejects developer mode.
Both integrations
are compiled out unless debug assertions are enabled, including release builds
with both features selected. Neither feature changes the normal app's profile,
palette, persistence, editor shortcuts or project recovery.

The shared Home debug launcher owns the disposable profile, connection file,
private Xvfb display on Linux, browser gateway and child process lifecycle. Use
that launcher rather than attaching the bridge to a working editing session.
Existing portable package markers override platform profile directories; do not
launch a portable package as a disposable development instance. Debug display
sessions select no-device audio backends for playback, audition and recording:
playback uses the existing wall clock and recording reports the localized error.

The viewer displays native frames and transparent DOM targets. Orca's own
browser Annotate and Draw tools own feedback. The app supplies no annotation
interface and does not send feedback messages. Standard egui AccessKit controls
provide current labels, roles and IDs; explicit registrations add source
locations for action menu items, and stable sequence/track/clip
paths for the visible timeline. Unregistered painted regions remain drawable.
Source locations are not claimed for every AccessKit node.

Subsecond only patches the binary crate. For debug `hot-reload` builds,
`main.rs` compiles the library source as its own module, so editor source
belongs to that crate. Normal builds and the headless CLI still use the library.
The existing `App`, project history, GPU objects and asynchronous jobs are
constructed once, outside `subsecond::call`; persistence and close handling keep
their current ownership. Only function-body edits are supported. Save work and
restart the disposable process before changing struct fields, enum layout,
dependencies, features or static initialization. Never apply a layout-changing
patch to a live editor.

| State or transition | Handling |
| --- | --- |
| Off | No plugin or remote input without debug assertions, feature and runtime opt-in. |
| Starting / connecting | Installation validates the SDK connection configuration; startup errors propagate. |
| Live | SDK publishes native captures with the matching current element metadata. |
| Offline / reconnect | SDK owns input leases, session IDs and stale-input rejection; browser retains its feedback. |
| Viewport / DPI change | Current native geometry is published with its captured frame; timeline targets are clipped to visible lanes. |
| Dynamic editor items | Sequence, track and clip IDs form paths; row position and label are not identities. |
| Compile error | Existing process and its unsaved work continue; no restart-on-save mechanism is installed. |
| Successful patch | A registered handler wakes idle egui; frame functions update without constructing a replacement app or GPU context. |
| Incompatible type edit | Requires an explicit restart after saving; automatic state migration is unsupported. |
| Hot-patch server disconnect | The editor keeps running; save work before restarting under a fresh Dioxus CLI session. |
| Duplicate launch / port collision | Shared launcher and SDK own session identity, exclusive connection files and ephemeral ports. |
| Held input / focus loss | SDK releases held input; existing timeline cancellation also handles lost focus. |
| Concurrent screenshot requests | SDK tags its captures and leaves unrelated screenshot events to their owner. |
| Unsaved work / cleanup | Existing discard, autosave and recording-finalization paths remain in the editor; launcher owns its children only. |

Linux verification uses an isolated Xvfb display and lavapipe. Compilation does
not prove a live Subsecond patch or Orca Annotate/Draw acceptance. Windows GPU,
hardware and hot-patch acceptance require PC testing; Windows Dioxus CLI builds
also require the stub RAX fix from DioxusLabs/dioxus#5967.

The Dioxus CLI must include the captured-crate graph fix from
[DioxusLabs/dioxus#5903](https://github.com/DioxusLabs/dioxus/pull/5903).
Stock CLI 0.7.10 includes unrelated workspace dependents in the patch graph:
editing the shared app source fails with `Missing rustc args for replay:
'jvid_cli'`, because the served app build never compiled that sibling binary.
Use a corrected developer CLI without changing the production workspace graph
or downgrading Subsecond.

The isolated Linux tool was built from official CLI commit
`57d6794ad60b949e5bd8aa282f6f8c3dc97a365e` with a ten-line backport of that
captured `.lib`/`.bin` guard. Its version is
`0.7.10 (57d6794+graph-0cf7878)`; the patch SHA-256 is
`0cf78788b104cbf118835521c4082273ddcfec078f00310096942621dda35f87`
and the binary SHA-256 is
`11e083a92c53cf7b892b92ad1c371d5b44fd2dc19db9400fdeb5acbed5be25a2`.
The source, patch, provenance and binary remain in the debug task's ticket
directory; the installed CLI is unchanged.
