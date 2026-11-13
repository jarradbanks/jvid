# updating

**Help → Check for Updates.** jvid checks once at startup (turn that off in the same window, it's `automatic_update_checks` in `settings.toml`). it never downloads, installs or restarts without a click.

- **windows installer:** **Download Update**, then **Install and Restart**. unsaved work gets a save prompt first. **Later** keeps the download for the session. it won't install mid-export, mid-recording or mid-model-download.
- **everything else** (portable, macos, linux, source builds): you get the notes and a download link. close jvid, replace the program files, keep `jvid-data`.

the installer only touches program files - projects, settings and models stay put.

updates are checked against signed metadata (ed25519, key baked into the build), and the installer is hash-checked twice: after download and right before it runs. an unsigned or mismatched download never runs.

## if something goes wrong

the result of the last update is in `update-result.txt` beside your settings (on windows, `%APPDATA%\jvid\config`), plus `update-result.installer.log` if the installer got that far. a failed update leaves the old install working - just launch it again.

on versions before 0.2.1, the windows updater could wrongly report "Editor did not exit". install 0.2.1 or newer by hand once to fix it.

## releasing

for maintainers. the flow is **develop → release/vX.Y.Z → main → tag → approve → publish**, and almost all of it is automated.

1. merge features into `develop`. normal PRs don't bump the version.
2. on main's actions page, run **Prepare release** (patch/minor/major). it branches `release/vX.Y.Z` off develop, bumps the version, drafts notes in `packaging/release-notes.md`, opens a PR to main and kicks off CI and Package on all three OSes.
3. tidy the notes (the `release-notes` skill helps; old examples are in `packaging/release-notes-examples/`). download the candidate and try it on each platform:

   ```sh
   gh run download "$RUN_ID" --repo jarradbanks/jvid --name release-candidate --dir candidate
   (cd candidate && sha256sum --check SHA256SUMS.txt)
   ```

4. merge the release PR into main with a **merge commit**. that tags `vX.Y.Z` and builds the release.
5. approve the `release` environment. it signs the update metadata, publishes `sources-vX.Y.Z` first, then the app release.

needs: repo variables `JVID_UPDATE_PUBLIC_KEY` and `JVID_RELEASE_ENABLED=true`, the `JVID_UPDATE_SIGNING_KEY` secret on the `release` environment, and actions allowed to create PRs. the signing seed never goes in code, args or logs.

**when it breaks:** rerun the same workflow run rather than starting a fresh one - prepare, tagging and publishing all pick up where they left off. rerun **failed jobs** on the original package run if signing or upload failed (its artifact lives seven days). never move or delete a published tag; ship a higher version instead. hotfixes go through develop like everything else, as a **patch** bump.

the asset names, metadata schema and target mapping live in `jvid_update_config::PACKAGES`.
