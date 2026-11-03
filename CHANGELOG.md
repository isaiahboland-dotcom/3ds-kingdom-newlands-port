# Changelog

## 1.0.0 — 2026-11-03

Initial versioned release of the Kingdom: New Lands fan port for Nintendo 3DS,
with a shared desktop backend and local asset conversion tools.

### Fixed

- Saving over existing SD-card slots now handles FAT rename restrictions. Writes
  flush and close before replacement, retain a backup on the SD filesystem, and
  attempt rollback on failure. Missing primary saves can recover from backups.
  The user confirmed the save fix works on hardware.
- Tree chopping and builder sounds use the worker position and distance rolloff,
  preventing work across the map from playing at full volume.
- Free archers rebalance every second after recruitment and losses. Tower guards
  and knight followers keep their assignments; odd free counts differ by one.
- Save migration and world-index validation reject incompatible or malformed data.
- Rendering submission, queued audio effects, audio decoding and mixing, and
  platform initialization have additional validation and failure handling.
- Simulation edge cases, asset parsing, and conversion tools have additional
  bounds checks and regression coverage.

### Build and distribution

- Workspace and 3DS package versions are now 1.0.0.
- Packaging fallback sources are pinned, publication checks are expanded, and
  Rust 1.88 is the declared minimum supported version.
- Converted game assets and asset-containing CIA/3DSX bundles remain local;
  releases of this repository contain source only. Supply your own game files.

### Validation and remaining checks

Workspace tests, Clippy with warnings denied, standalone save replacement tests,
and the 3DS release build pass. Hardware save behavior is confirmed by the user;
audio range and archer balancing still need in-game confirmation. Broader GPU/DSP
soak testing and other follow-ups are tracked in
[the review status](docs/fix-review-status.md) and
[the hardware follow-up](docs/hardware-followup-2026-11-03.md).
