# Iki brand assets

The mark is the kanji **息** (*iki*, "breath") brushed in cream on an indigo tile. The wordmark is lowercase mono **`iki`** with square indigo dots and the 息 seal beside it, a sibling of the Hibi mark. Both were generated with Nano Banana Pro, then recoloured and cut out by the scripts below.

## Palette

| Role | Hex | Use |
|------|-----|-----|
| Indigo (藍) | `#2E4A6B` | tile, seal, the dots on the `i`s |
| Light indigo | `#7C9AC0` | accents drawn on the black pill |
| Cream | `#F0E8D5` | the 息, the wordmark on dark |
| Sumi ink | `#1B1B1A` | the wordmark on light |

## Files

| File | Use |
|------|-----|
| `iki-mark-source.png` | generated mark, the input to `make-icons.swift` |
| `iki-lockup-source.png` | generated lockup in its original wisteria, the input to `make-wordmark.swift` |
| `AppIcon.icns` | app icon, 16–1024 px on the macOS icon grid |
| `StatusIcon.png`, `StatusIcon@2x.png` | menu bar template glyph, 18 and 36 px |
| `iki-mark.png` | 1024 px square mark without rounding, for avatars and social |
| `iki-wordmark.png`, `iki-wordmark-dark.png` | README wordmark on transparency, sumi and cream letters |

## Regenerating

```sh
swift scripts/make-icons.swift assets/brand/iki-mark-source.png 2E4A6B F0E8D5 assets/brand
swift scripts/make-wordmark.swift assets/brand/iki-lockup-source.png A394C7 2E4A6B assets/brand
```

`make-icons.swift` splits each pixel of the mark into glyph coverage and paper grain, so any tile and glyph colour can be applied without regenerating the art. `make-wordmark.swift` unmixes the paper into alpha so letter edges carry the letter colour, not a paper halo.
