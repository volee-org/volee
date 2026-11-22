import { svelte } from "@sveltejs/vite-plugin-svelte";
import { defineConfig } from "vitest/config";

// The daemon serves the built bundle, so everything is referenced relatively
// and the output is a flat, predictable set of files.
export default defineConfig({
  plugins: [svelte()],
  base: "./",
  build: { outDir: "dist", emptyOutDir: true, target: "es2022" },
  test: { environment: "node", include: ["tests/**/*.test.ts"] },
  // `npm run dev` proxies the API to a daemon on the default port.
  server: { proxy: { "/api": "http://127.0.0.1:47835" } },
});
