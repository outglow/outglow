import { resolve } from "node:path";
import { defineConfig } from "vitest/config";

// GitHub Pages serves the project site under /outglow/; OUTGLOW_BASE overrides
// it (e.g. "/" for a custom domain or a local preview).
export default defineConfig({
  base: process.env.OUTGLOW_BASE ?? "/outglow/",
  build: {
    outDir: "dist",
    emptyOutDir: true,
    rollupOptions: {
      input: {
        index: resolve(import.meta.dirname, "index.html"),
        install: resolve(import.meta.dirname, "install.html"),
        flasher: resolve(import.meta.dirname, "flasher/index.html"),
      },
    },
  },
  test: {
    include: ["test/**/*.test.ts"],
  },
});
