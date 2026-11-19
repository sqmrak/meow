# pictures

icons, launch images, glyphs and sprites are made by meow itself: png reading and writing, lanczos resizing, rounded corners, alpha compositing and an svg rasterizer are built in. no pillow, rsvg or ffmpeg.

each table makes one step: `icons-NAME`, `launch-NAME`, `glyphs-NAME` or `image-NAME`. its outputs are the files it writes. the outputs live in the source tree, so the step keeps them until the new ones are written and writes each through a temporary name, a failed run leaves the old file. `needs = [...]` and the step names work like any other step, for example `needs = ["icons-plain"]` on a product.

## icons

every size springboard asks for, from one master.

```toml
[icons.plain]
master = "art/master.png"
out = "app/icons"
preset = "ios"
sizes = { "Icon-master.png" = 512 }
round = 0.18
overlay = "art/gloss.png"
overlay_sizes = [57, 72]
drop_alpha = true
```

- `preset = "ios"`: `Icon.png` 57 to `Icon-83.5@2x.png` 167, 11 names, the ones ios 5 to 15 look for
- `sizes`: more names, or a new size for a preset name
- `round`: rounded corners, the radius as a fraction of the width
- `overlay`: laid over, scaled to each size. `overlay_sizes` limits it to these edges, every size when left out
- `drop_alpha`: the default unless `round` is set

## launch images

a flat color, with the icon in the middle when one is named.

```toml
[launch.main]
out = "resources/launch"
background = "#8455c1"
icon = "art/master.png"
icon_points = 96
icon_round = 0.18
images = [{ name = "Default@2x.png", width = 320, height = 480, scale = 2 }]
```

`icon_points` is the edge in points, times the scale of each image. a screen without a launch image of its size runs the app letterboxed, so list every size the project supports.

## glyphs

svg or android vector files, square, at every scale.

```toml
[glyphs.ui]
src = ["art/glyphs/*.svg"]
items = [{ from = "art/ic_news_24.xml", name = "feed" }]
out = "app/icons/glyphs"
prefix = "glyph-"
suffix = "-light"
size = 24
scales = [1, 2, 3]
color = "#8f939d"
```

- the file stem is the name. `items` names a file one by one
- `size` is in points at scale 1. `scales` makes `name.png`, `name@2x.png`, `name@3x.png`
- `color` replaces every fill and stroke color

## image

one picture from another.

```toml
[image.mascot]
from = "art/mascot.png"
to = "app/icons/Icon-mascot.png"
crop_alpha = 32
fit = [24, 24]
canvas = [24, 24]
round = 0.18
scales = [1, 2]
```

- `crop_alpha`: crop to the pixels with at least this alpha, first
- `fit`: inside the box, never enlarged. `size = [w, h]` gives an exact size instead
- `canvas`: centered on a transparent canvas, `background = "#rrggbb"` fills it
- `scales`: the sizes are multiplied and the file gets `@2x`

## formats

png input: every color type and bit depth, palette and tRNS, not interlaced and not apple optimized (CgBI). jpeg input is refused with a message, convert it to png once.

svg: path (all commands, arcs included), circle, ellipse, rect, line, polyline, polygon, g, fill, stroke with width, caps and joins, fill-rule, opacity, viewBox starting at 0 0, `style` declarations. a transform, gradient, clip path or text is an error.

output is png, rgb when no pixel is transparent.

checked against pillow and rsvg-convert on 100 files of real projects (icons, deb icons, launch images, 72 glyphs): the visible difference is at most 10 of 255 for icons and launch images, 28 on the edge pixels of a few glyphs, mean under 1.
