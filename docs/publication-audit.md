# Publication audit

Reviewed on 2026-11-02.

## Files intended for publication

The review covered Git-tracked files, staged blobs, and non-ignored untracked files. The repository had no commits, so there was no committed history to clean. Local private directories were retained for development and command verification.

No proprietary game binaries, decompiled C# files, extracted images, audio, fonts, scenes, or generated world data were found among the files intended for publication. No complete public file matched the SHA-256 of a file under the local `gamefiles/`, `reference/`, or `assets/` directories. A comparison of exact lines of at least 80 characters found no matches between public Rust files and local decompiled C#; this is a limited copy-detection check, not proof of independent authorship.

Rust code and documentation identify original class names and observed behavior. The implementation was developed with decompiled references; it should not be described as a clean-room implementation. These checks cannot establish the provenance of every implementation or settle rights to translated or adapted code. The repository's MIT license does not relicense the original game or its content.

The CIA configuration identifies the devkitPro homebrew template it was based on. Rust dependencies are fetched through Cargo, and external conversion/packaging tools remain outside the public file set. Dependency and tool licenses continue to apply independently of this project's license.

## Protections

`.gitignore` excludes game installations, decompiled/ripped references, converted assets, 3DS RomFS and SD staging data, binaries, downloaded tools, saves, and local credentials/configuration directories. Mod loading, tuning patches, replacement art/audio, the modkit exporter, and the example mod have been removed.

Run `python3 tools/check_public_files.py` before publication. It checks both local Git-visible files and staged contents, rejects private paths and common asset/binary formats, and detects common decompiler signatures. CI runs the same check. This guards against accidental inclusion; force-adding files or putting proprietary content into otherwise allowed text files still requires human review.

Publish source only. Locally generated `.3dsx` and `.cia` packages contain converted game content and must not be attached to public releases. Do not upload a ZIP of the entire working directory, which includes ignored private files.

## Verification

The following checks passed locally:

- Workspace builds with all targets and locked dependencies.
- Workspace tests: 92 passed.
- Workspace Clippy with all targets and warnings treated as errors.
- ShellCheck for both setup and CIA scripts; setup help output.
- `tools/setup.sh` using the existing local AssetRipper export; the generated world fingerprint matches the expected Steam Linux build.
- Direct asset conversion from the same export.
- `pack3ds`, followed by a nightly `cargo 3ds build --release` with devkitPro on `PATH`.
- `tools/cia/build.sh`, including banner/icon/jingle generation and successful CIA packaging, using locally available packaging tools.
- Publication file check and temporary-repository probes confirming that private staged blobs are rejected even if deleted locally.
- AssetRipper 2.0.0 archive checksum constants match all six digests in the upstream GitHub release metadata.

The converter reports an unsupported numeric field in an unused Saitek controller profile and skips that profile; conversion completes and produces the expected world fingerprint.

A fresh AssetRipper download/export, first-time source builds of makerom/bannertool, desktop graphical play, and console installation/gameplay were not exercised in this audit. The GitHub Actions workflow was updated but has not run on GitHub here. Hardware audio, reflections, stereo, and Old/New 3DS performance still need validation. Existing formatting differences elsewhere in the workspace remain; CI does not enforce workspace-wide rustfmt.
