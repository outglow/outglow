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
        bulbs: resolve(import.meta.dirname, "bulbs.html"),
        why: resolve(import.meta.dirname, "why.html"),
        sensing: resolve(import.meta.dirname, "sensing.html"),
        flasher: resolve(import.meta.dirname, "flasher/index.html"),
        console: resolve(import.meta.dirname, "console/index.html"),
      },
    },
  },
  test: {
    include: ["test/**/*.test.ts"],
  },
});
