import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";
import { defineConfig } from "vitest/config";
import brand from "../../branding.json";

export default defineConfig({
  plugins: [react(), tailwindcss()],
  clearScreen: false,
  define: { __BRAND__: JSON.stringify(brand) },
  server: { port: 1420, strictPort: true },
  build: { target: "es2022", sourcemap: false },
  test: { environment: "jsdom", include: ["src/**/*.test.ts", "src/**/*.test.tsx"] },
});
