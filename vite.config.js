import { defineConfig } from "vite";

export default defineConfig({
  clearScreen: false,
  build: {
    outDir: "dist-web",
    emptyOutDir: true,
    target: ["es2021", "chrome105", "safari13"]
  },
  server: {
    strictPort: true,
    watch: {
      ignored: ["**/src-tauri/**"]
    }
  }
});
