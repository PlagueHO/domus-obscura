import react from "@vitejs/plugin-react";
import { defineConfig } from "vite";

export default defineConfig({
  root: "local-test",
  plugins: [react()],
  build: {
    outDir: "../dist/local-test",
    emptyOutDir: true,
    rollupOptions: {
      output: {
        entryFileNames: "assets/local-test.js",
        assetFileNames: "assets/local-test[extname]",
      },
    },
  },
});
