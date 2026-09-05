import { defineConfig } from "vite";
import { sveltekit } from "@sveltejs/kit/vite";

// https://vite.dev/config/
export default defineConfig({
  plugins: [sveltekit()],

  // `wails3 dev` starts this server and points the webview at it; the port
  // must match VITE_PORT in Taskfile.yml.
  clearScreen: false,
  server: {
    host: "127.0.0.1",
    // @ts-expect-error process is a nodejs global
    port: Number(process.env.WAILS_VITE_PORT) || 9245,
    strictPort: true,
  },
});
