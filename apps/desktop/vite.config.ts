import { defineConfig } from 'vite';
import { fileURLToPath, URL } from 'node:url';

export default defineConfig({
  root: fileURLToPath(new URL('./ui', import.meta.url)),
  publicDir: fileURLToPath(new URL('./ui/vendor', import.meta.url)),
  clearScreen: false,
  build: {
    outDir: fileURLToPath(new URL('./dist', import.meta.url)),
    emptyOutDir: true,
  },
});
