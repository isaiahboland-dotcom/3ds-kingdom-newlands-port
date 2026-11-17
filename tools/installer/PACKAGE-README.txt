Kingdom: New Lands for Nintendo 3DS - installer
===============================================

An unofficial fan port. You need your own PC copy of Kingdom: New Lands
(Steam or GOG, Windows or Linux build); nothing from the game is in this
package. The installer converts your copy on this computer and puts the
result on your 3DS's SD card.

1. Put the 3DS's SD card in this computer.
2. Run kingdom-installer (kingdom-installer.exe on Windows).
3. Pick the Kingdom New Lands folder (the one that contains Kingdom_Data)
   and the SD card, tick .3dsx and/or .cia, and press Install.
   The first run downloads AssetRipper (about 100 MB) and takes a few
   minutes; later runs are faster.
4. Eject the card. On the 3DS:
   - .3dsx: start Kingdom from the Homebrew Launcher.
   - .cia:  install cias/kingdom.cia with FBI, then start it from the HOME
            Menu. (.cia is experimental.)

Sound needs the console's DSP firmware at 3ds/dspfirm.cdc on the card (run
the DSP1 homebrew once). The installer tells you if it isn't there.

Your saves live in 3ds/kingdom-3ds/ on the card and are never overwritten.

If something fails the window shows why. Work files are kept in
  Windows: %LOCALAPPDATA%\kingdom-3ds-installer
  Linux:   ~/.local/share/kingdom-3ds-installer
and assetripper.log there helps when reporting a problem.

Don't keep this folder's bin/ and data/ apart from the installer: it looks
for them next to itself.

Project and source: https://github.com/isaiahboland-dotcom/3ds-kingdom-newlands-port
Licence: MIT for the project's code (see LICENSE). Does not cover the game.
