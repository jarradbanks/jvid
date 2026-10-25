Choose the download for your system:

- Windows x86_64: **setup.exe** installs for your user with shortcuts and an uninstaller; **portable.zip** runs after extraction.
- macOS Apple Silicon: **.dmg** contains the app to drag into Applications; **portable.zip** keeps data beside its app.
- Linux x86_64 (Ubuntu 22.04 or newer): extract **portable.tar.gz** and run AppRun on a desktop with a Vulkan driver.

Required FFmpeg playback/export libraries and the integrated speech engine are included. Whisper **models are optional downloads** offered at first launch and later under Help > Optional Components. Editing and export work without them.

Keep the entire portable folder together. Portable settings and models live in `jvid-data` at the package root; macOS keeps it beside the .app. Preserve that folder when replacing program files. See the [installation and packaging guide](https://github.com/jarradbanks/jvid/blob/main/docs/packaging.md) for paths, upgrade instructions, supported systems, source/licensing details and validation limits.

Verify downloads using SHA256SUMS.txt. The corresponding-source archive accompanies the GPL runtime distribution.

This is a draft until the maintainer records clean-machine validation and completes signing/notarization and the source review.
