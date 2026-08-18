import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';
import tailwindcss from '@tailwindcss/vite';

export default defineConfig({
  plugins: [tailwindcss(), react()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    watch: {
      // The Rust bridge rewrites these on every capture and throughout a tailoring run,
      // and they sit inside the Vite root. Without this the dev server issues a full page
      // reload mid-capture and mid-pipeline, which is what makes the desktop app flicker.
      ignored: [
        '**/data/**',
        '**/resume/**',
        '**/src-tauri/target/**',
        '**/dist/**',
      ],
    },
  },
});
