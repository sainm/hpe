import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

const apiTarget = process.env.HPA_API_URL ?? "http://127.0.0.1:8787";

export default defineConfig({
  plugins: [react()],
  server: {
    proxy: {
      "/api": {
        target: apiTarget,
        changeOrigin: true,
      },
    },
  },
  build: {
    outDir: "dist",
    sourcemap: true,
  },
});
