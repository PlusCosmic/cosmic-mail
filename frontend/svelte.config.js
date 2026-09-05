// Wails serves a static bundle from the binary and has no Node.js server, so
// the app is built with adapter-static in SPA mode (fallback to index.html).
// See: https://svelte.dev/docs/kit/single-page-apps
import adapter from "@sveltejs/adapter-static";
import { vitePreprocess } from "@sveltejs/vite-plugin-svelte";

/** @type {import('@sveltejs/kit').Config} */
const config = {
  preprocess: vitePreprocess(),
  kit: {
    // The TypeScript that `wails3 generate bindings` writes.
    alias: { $bindings: "./bindings" },
    adapter: adapter({
      // Wails embeds frontend/dist (see main.go); `build/` is the Wails
      // build-asset directory at the repository root. The bundle goes one
      // level down because adapter-static empties its output directory on
      // every build, and Go's embed needs the tracked frontend/dist/.gitkeep
      // to survive so the Go checks work on a checkout with no frontend
      // build.
      pages: "dist/app",
      assets: "dist/app",
      fallback: "index.html",
    }),
  },
};

export default config;
