# App icon artwork

Quill's iPhone app icon and the shortlist it was chosen from. They were generated on 2026-11-01 with fal.ai, picked from two rounds of candidates (19 images), and are kept here with the prompts that made them so any of them can be redone or varied.

Recraft V3 and FLUX 1.1 [pro] (through the API) are offered for commercial use, unlike FLUX [dev], which is non-commercial; before the App Store, confirm that on fal.ai's model pages, and check that no existing mark looks like the icon. Each file also carries the generator's C2PA provenance manifest.

To change the icon, replace the three PNGs in `Quill/Assets.xcassets/AppIcon.appiconset` (1024 by 1024, no transparency). For a layered icon with real light, dark, and tinted variants, split one of the vector designs into background and feather layers in Icon Composer (Xcode 27).

## The shortlist

### 08b: gold foil on charcoal (the app icon)

`shortlist/08b-foil-charcoal.jpg`. The icon in `Quill/Assets.xcassets/AppIcon.appiconset`: the 1024 by 1024 original as `AppIcon.png`, the same image as the dark appearance (it's already dark), and a grayscale copy as the tinted appearance (`sips -m` with the Generic Gray profile).

- Model: `fal-ai/flux-pro/v1.1` on fal.ai
- Prompt: Square artwork filling the entire frame edge to edge, no border, no frame, no tile edges: a feather quill hot-stamped in shiny gold foil into dark charcoal textured cotton paper, debossed, soft directional light catching the foil, very high contrast gold on near-black, minimal, diagonal, no text.
- Settings: `image_size`: `"square_hd"`, `num_images`: `1`, `safety_tolerance`: `"2"`

### 05d: cream and gold feather on midnight blue

`shortlist/05d-feather-navy.svg`. Vector. The generator drew a frame around the art; it was removed by cropping the SVG's `viewBox` to `70 70 1908 1908` (of 2048), not by regenerating.

- Model: `fal-ai/recraft/v3/text-to-image` on fal.ai
- Prompt: Full-bleed square app icon artwork, no frame or border: an elegant cream feather quill with a few fine barb lines and a soft gold edge, diagonal, on a deep midnight blue background, minimal and refined, no text.
- Settings: `style`: `"vector_illustration"`, `image_size`: `"square_hd"`, `colors`: `[{"r": 16, "g": 28, "b": 66}, {"r": 246, "g": 233, "b": 204}, {"r": 201, "g": 162, "b": 83}]`, `background_color`: `{"r": 16, "g": 28, "b": 66}`

### 05b: cream feather on crimson (warmer alternative)

`shortlist/05b-feather-crimson.svg`. Vector, framed the same way and cropped the same way.

- Model: `fal-ai/recraft/v3/text-to-image` on fal.ai
- Prompt: Full-bleed square app icon artwork, no frame or border: an elegant cream feather quill with a few fine barb lines, diagonal, on a rich crimson background with a subtle darker crimson vignette, minimal and refined, no text.
- Settings: `style`: `"vector_illustration"`, `image_size`: `"square_hd"`, `colors`: `[{"r": 139, "g": 16, "b": 32}, {"r": 96, "g": 8, "b": 20}, {"r": 246, "g": 233, "b": 204}]`, `background_color`: `{"r": 139, "g": 16, "b": 32}`

### 08d: cream feather on crimson letterpress (warmer alternative)

`shortlist/08d-letterpress-crimson.jpg`. Pairs with 05b's palette.

- Model: `fal-ai/flux-pro/v1.1` on fal.ai
- Prompt: Square artwork filling the entire frame edge to edge, no border, no frame, no tile edges: a cream feather quill pressed into thick deep crimson cotton letterpress paper, raking side light with crisp shadows in the deboss, high contrast pale feather on dark red, minimal, diagonal, no text.
- Settings: `image_size`: `"square_hd"`, `num_images`: `1`, `safety_tolerance`: `"2"`
