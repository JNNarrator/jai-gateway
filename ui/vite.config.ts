import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";
import { fileURLToPath, URL } from "node:url";

// Tauri 固定 devUrl=5173：strictPort 防止漂移
export default defineConfig({
  plugins: [react(), tailwindcss()],
  clearScreen: false,
  server: { host: "127.0.0.1", port: 5173, strictPort: true },
  build: { target: "es2021" },
  resolve: { alias: { "@": fileURLToPath(new URL("./src", import.meta.url)) } },
});
