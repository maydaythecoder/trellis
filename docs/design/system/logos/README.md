# Logos

The TRELLIS mark is a 3×3 lattice — six 1.5px lines at `space-6` pitch with a port dot at every crossing, one crossing lit in `signal` with a ring. It is the system's connector language made into a sign: lines connect, the dots are where things meet, the lit one is where you are.

- `trellis-mark-dark.svg` — ink `#ececf1`, lit `#ffb224`. Use on `ground` and `panel` in the Dark theme.
- `trellis-mark-light.svg` — ink `#141418`, lit `#9a5b00`. Use on the Daylight paper grounds.
- `trellis-mark-mono.svg` — one ink, `#ececf1`, no lit crossing. For places that take one colour (tray icons, print on dark). Single-ink SVG shown through `<img>` cannot inherit colour; pick the file for the ground, or inline the SVG and set `stroke`/`fill` to `currentColor`.
- `trellis-wordmark-dark.svg`, `trellis-wordmark-light.svg` — the small mark plus "TRELLIS" in JetBrains Mono 500, 36px, 0.04em tracking. The word is live `<text>`, not outlines: it renders in JetBrains Mono where the font is installed or loaded, otherwise in the system mono. Outline it before sending to print.
- `favicon.svg` — the dark mark on `#0b0b0d`.
- `app-icon-512/256/128/32/16.png` — the dark mark on a `#0b0b0d` rounded square (`radius-md` scaled: 8px at 48px). The 16 and 32 are for the OS title bar and tray; 512 is the source for store listings.

Clear space around the mark: one lattice cell (a third of its width). Minimum size 16px. Never recolour the lit crossing to anything but `signal` or the theme's `signal` value; never add a gradient or shadow.
