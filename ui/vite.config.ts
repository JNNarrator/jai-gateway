import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// Tauri 固定 devUrl=5173：strictPort 防止漂移
export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: {
    port: 5173,
    strictPort: true,
  },
  build: {
    target: "es2021",
  },
});
