# SD-card follow-up: November 3, 2026

Read-only review of `/run/media/trash/C7A0-6103/3ds/kingdom-3ds`.
No Kingdom log file was found. Saves were copied to the ignored local
`target/sd-review-2026-11-03/` directory; the card was not modified.

## Findings and fixes

- Worker sound events carried a target object ID, but the scene queried the
  citizen table with it. Missing citizens produced an unpositioned sound at
  full listener volume; coincident IDs could position sounds on unrelated units.
  Workers now emit a separate sound cue containing their actual position for
  each completed stroke, before a chopped tree can disappear. The existing
  emitter's spatial rolloff silences construction/chopping beyond 11 world units.
- Save temporary files were complete and loadable, but existing destination
  files remained unchanged. The slot selector remained 0 while its temporary
  file contained 2. This matches the SD rename operation rejecting an existing
  destination. Replacement now flushes and closes the temporary file, moves
  the previous save to `.sav.bak`, then installs the new save, rolling back on
  failure. Reads recover from `.bak` if interrupted while the main file is absent.
  A successful replacement retains the previous backup. Slot deletion also
  deletes its backup and temporary file.
- Free-archer distribution already divided hunters into halves at dusk, but
  included knight followers. Both distribution and tower recruitment now
  exclude followers. The one-second assignment tick also redistributes free
  hunters, so recruits and losses do not leave sides unbalanced until dusk.
  An odd hunter count differs by one. Tower guards and knight bands retain
  their assignments; total visible archers need not be equal on asymmetric defenses.

## Save inspection

All six game files (three slots and their temporary files) loaded successfully
against the current world fingerprint `d88fcb9c`. Slot 1 is version 4 and migrated
successfully; other files are version 5. Slot 2's saved look is a king, while
its temporary file and slot 3 contain the queen with pattern 4 and emblem 1.
The requested appearance was therefore preserved in slot 3. Nothing was promoted
or overwritten on the SD card during inspection.

## Verification

Workspace tests and Clippy pass. Standalone host tests of the 3DS save-file
helper exercise FAT-style refusal to overwrite, install failure and rollback,
backup recovery, and subsequent saving. Archer tests cover recruitment, losses,
odd/even counts, and excluding knight/tower specialists. A simulation repair test
checks positioned worker events; the scene test checks emitter-range attenuation.
The updated CIA and 3DSX are built locally. The user confirmed that saving now works on hardware. In-game audio range
and nighttime archer behavior still need a hardware retest.
