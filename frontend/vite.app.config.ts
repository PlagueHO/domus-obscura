import react from "@vitejs/plugin-react";
import { defineConfig } from "vite";

export default defineConfig({
  root: "app",
  plugins: [react()],
  build: {
    outDir: "../dist-app",
    emptyOutDir: true,
    assetsInlineLimit: 0,
  },
});
