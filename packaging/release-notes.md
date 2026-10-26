Changes in 0.2.1

- Fix Windows updater when the editor has already exited

**Windows users updating from 0.2.0 or earlier:** download and run the `jvid-0.2.1-windows-x86_64-setup.exe` installer manually once. Older installed update helpers can incorrectly report “Editor did not exit” before starting the installer. Install into the same directory to replace that helper and keep your settings, projects and models. Future in-app updates will use the repaired helper.

Choose the download for your system:

- Windows x86_64: **setup.exe** installs for your user with shortcuts and an uninstaller; **portable.zip** runs after extraction.
- macOS Apple Silicon: **.dmg** contains the app to drag into Applications; **portable.zip** keeps data beside its app.
- Linux x86_64 (Ubuntu 22.04 or newer): extract **portable.tar.gz** and run AppRun on a desktop with a Vulkan driver.

Required FFmpeg playback/export libraries and the integrated speech engine are included. Whisper **models are optional downloads** offered at first launch and later under Help > Optional Components. Editing and export work without them.

Keep the entire portable folder together. Portable settings and models live in `jvid-data` at the package root; macOS keeps it beside the .app. Preserve that folder when replacing program files. See the [installation and packaging guide](https://github.com/jarradbanks/jvid/blob/main/docs/packaging.md) for paths, upgrade instructions, supported systems, source/licensing details and validation limits.

Download the app packages above to install jvid. The separate `sources-*.json`, `.tar.gz.partNNN`, source manifest and reconstruction files provide corresponding source for review/rebuilding; they are not app installers. Verify downloaded files using `SHA256SUMS.txt`.

Release candidates are tested before merge; the final main tag build is published automatically after the maintainer approves the protected release job.

Platform acceptance and signing/notarization status are documented in the installation and packaging guide.
