# Brand assets

The name comes from the French *volée*: a flight of birds moving together. The
logo is that flight: seven birds in V formation that draw the "v" of volee,
with the leading bird in mauve. One lead, many workers, moving together.

## Files

| File | Use |
| --- | --- |
| `mark-dark.svg`, `mark-light.svg` | The mark alone. |
| `wordmark-dark.svg`, `wordmark-light.svg` | The mark and the name: README header, slides, posts. |
| `icon.svg` | The mark on a dark rounded square: favicon (copied to `docs/theme/favicon.svg`) and app icon. |
| `avatar.svg`, `avatar.png` | Square avatar, 512 x 512 px, for platforms that round or crop avatars themselves (GitHub, chat apps). |
| `social-preview.svg`, `social-preview.png` | Repository social preview card, 1280 x 640 px. |

Files ending in `-dark` go on dark backgrounds and files ending in `-light` on
light ones. On GitHub, switch between them with a `<picture>` element:

```html
<picture>
  <source media="(prefers-color-scheme: dark)" srcset="assets/brand/wordmark-dark.svg">
  <img alt="volee" src="assets/brand/wordmark-light.svg" width="360">
</picture>
```

## Colors

The palette is [Catppuccin](https://catppuccin.com/palette): Mocha on dark
backgrounds, Latte on light ones.

| Role | Mocha (dark) | Latte (light) |
| --- | --- | --- |
| Birds | lavender `#b4befe` | lavender `#7287fd` |
| Leading bird | mauve `#cba6f7` | mauve `#8839ef` |
| Name | text `#cdd6f4` | text `#4c4f69` |
| Background | base `#1e1e2e` | base `#eff1f5` |
| Links and highlights | blue `#89b4fa` | blue `#1e66f5` |

## Usage rules

- Write the name in lowercase, even at the start of a sentence: volee.
- Use the files as they are. Do not recolor, stretch, rotate or outline the
  mark, add shadows or gradients, or change the number or the order of the
  birds. The leading bird is always mauve.
- Use the `-dark` files on dark backgrounds and the `-light` files on light
  ones. On a photo or a busy background, use `icon.svg`.
- Leave clear space around the mark and the wordmark of at least half the
  height of the mark.
- Minimum widths: 24 px for the mark, 120 px for the wordmark. Below that, use
  `icon.svg`, which stays readable down to 16 px.
- The letters of the wordmark are drawn, not typed: no font is needed. Do not
  set "volee" next to the mark in another typeface.

## Editing and rendering

The SVG files are written by hand and kept small: every file draws the mark
with the same two paths, placed with a `transform`, with the leading bird at
the origin. Edit the source, then render the PNG files with
[librsvg](https://gitlab.gnome.org/GNOME/librsvg):

```sh
rsvg-convert -w 512 -h 512 assets/brand/avatar.svg -o assets/brand/avatar.png
rsvg-convert -w 1280 -h 640 assets/brand/social-preview.svg -o assets/brand/social-preview.png
rsvg-convert -w 64 -h 64 assets/brand/icon.svg -o docs/theme/favicon.png
cp assets/brand/icon.svg docs/theme/favicon.svg
```

The social preview tagline is set in Avenir Next and the address in JetBrains
Mono; render it on a machine that has both fonts, or the text falls back to
another sans-serif. The organization profile in
[volee-org/.github](https://github.com/volee-org/.github) keeps its own copy of
the wordmarks: update it when the wordmarks change.
