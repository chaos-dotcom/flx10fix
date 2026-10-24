# FLX10FIX

Fix the DDJ-FLX10 **"NO AUDIO DRIVER"** jog-display message on macOS.

## What this is

The DDJ-FLX10 Setting Utility ships a background helper
(`com.pioneerdj.DDJ-FLX10AutoLauncherd`) that keeps the host <-> unit handshake
alive. AlphaTheta's installer has carried a `com.apple.quarantine` extended
attribute on that helper's LaunchAgent since 2022. Current macOS refuses to load
quarantined agents, so the helper silently stops working — typically right after
a macOS update — and the unit shows "NO AUDIO DRIVER" until you open the utility
by hand.

`flx10fix` checks for exactly that and repairs it.

## Usage

    flx10fix            # status report
    flx10fix check      # same
    flx10fix fix        # repair (asks for your password)
    flx10fix fix --dry-run

Exit codes: `0` healthy / fixed, `1` fixable or remaining problems, `2` usage error.

## Notes

- Reinstalling the Setting Utility from AlphaTheta's current download re-plants
  the same quarantine flag (the file is unchanged); run `flx10fix fix` again.
- Only the quarantine attribute on the vendor's files is touched; nothing else
  is modified.
- Build & install: `cargo install --path .`
