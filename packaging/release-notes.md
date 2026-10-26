Changes in 0.2.0

- Wait for asynchronous bin fixture decoding before requiring idle UI
- Update grouped transcript snapshots for copy and export controls
- Update shortcut dialog snapshot for subtitle export action
- Add transcript copy and UTF-8 text export
- Reuse persistent recognition between subtitles and Transcript
- feat(app): enable draggable dock tabs and layout reset (#482)
- Add standalone subtitle SRT export (#485)

Choose the download for your system:

- Windows x86_64: **setup.exe** installs for your user with shortcuts and an uninstaller; **portable.zip** runs after extraction.
- macOS Apple Silicon: **.dmg** contains the app to drag into Applications; **portable.zip** keeps data beside its app.
- Linux x86_64 (Ubuntu 22.04 or newer): extract **portable.tar.gz** and run AppRun on a desktop with a Vulkan driver.

Required FFmpeg playback/export libraries and the integrated speech engine are included. Whisper **models are optional downloads** offered at first launch and later under Help > Optional Components. Editing and export work without them.

Keep the entire portable folder together. Portable settings and models live in `jvid-data` at the package root; macOS keeps it beside the .app. Preserve that folder when replacing program files. See the [installation and packaging guide](https://github.com/jarradbanks/jvid/blob/main/docs/packaging.md) for paths, upgrade instructions, supported systems, source/licensing details and validation limits.

Download the app packages above to install jvid. The separate `sources-*.json`, `.tar.gz.partNNN`, source manifest and reconstruction files provide corresponding source for review/rebuilding; they are not app installers. Verify downloaded files using `SHA256SUMS.txt`.

Release candidates are tested before merge; the final main tag build is published automatically after the maintainer approves the protected release job.

Platform acceptance and signing/notarization status are documented in the installation and packaging guide.
