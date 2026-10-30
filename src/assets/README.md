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

The macOS source was edited with the built-in image-generation tool. The edit
prompt requested the existing red broken-circle logo, enlarged on a single
near-black macOS rounded-square tile with transparent outer corners, no inset
black square, no gray frame, and no added text or symbols.
