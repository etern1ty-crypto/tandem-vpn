import { defineConfig } from "vite";

// Tauri expects a fixed port and the dist output one level up in `dist/`.
export default defineConfig({
  root: ".",
  clearScreen: false,
  server: {
    port: 5173,
    strictPort: true,
  },
  build: {
    outDir: "dist",
    emptyOutDir: true,
    target: "es2021",
  },
});
