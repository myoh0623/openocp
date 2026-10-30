# App icons

`AppIcon.png` is the original in-app and Windows artwork.
`AppIcon-macOS.png` is the macOS-specific rounded tile with transparent outer
corners. It is the source for `src-tauri/icons/icon.icns`; keep the other platform
icons unchanged when regenerating it.

From the repository root:

```sh
npm run tauri icon -- src/assets/AppIcon-macOS.png --output .build/macos-icons
cp .build/macos-icons/icon.icns src-tauri/icons/icon.icns
```

The macOS source was edited with the built-in image-generation tool, using
`AppIcon.png` as the edit target and requesting a transparent background. The
exact edit prompt was:

> Use case: precise-object-edit. Asset type: production macOS Dock app icon, square 1024 x 1024 transparent PNG. Edit target: the attached current Remote OCP icon. Preserve the exact recognizable red broken circular OpenShift symbol, its orientation, its two red tones and geometry; do not redesign the logo, add text, or add any other symbols. Change only its background, framing, and resolution: put the large centered red symbol on a single flat near-black (#080808) rounded-square macOS icon tile. Tile bounds approximately x=80 to 944, y=80 to 944 in the 1024 square, smooth macOS squircle rounded corners (corner radius roughly 195), genuinely transparent pixels outside the tile. Make the symbol large: approximately 790 pixels wide, leaving only about 36 pixels breathing room inside the tile on each side; keep the complete symbol visible. The black behind the logo must seamlessly fill the entire rounded tile, with NO smaller inset black square, NO gray or white surrounding tile or border, NO drop shadow, NO glossy effects, NO bevel. Sharp clean flat antialiased edges, crisp high resolution, centered. Deliver only the icon asset, no mockup, no grid, no caption.
