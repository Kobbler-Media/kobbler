import { sveltekit } from '@sveltejs/kit/vite';
import adapter from '@sveltejs/adapter-static';
import { vitePreprocess } from '@sveltejs/vite-plugin-svelte';
import { defineConfig, loadEnv } from 'vite';
import { fileURLToPath } from 'url';
import path from 'path';

const __dirname = path.dirname(fileURLToPath(import.meta.url));

export default defineConfig(({ mode }) => {
  const env = loadEnv(mode, process.cwd(), '');
  return {
    plugins: [
      sveltekit({
        preprocess: vitePreprocess(),
        adapter: adapter({ fallback: 'index.html' })
      })
    ],
    resolve: {
      alias: {
        'foliate-js/view.js': path.resolve(__dirname, 'node_modules/foliate-js/view.js')
      }
    },
    optimizeDeps: {
      include: ['foliate-js/view.js']
    },
    worker: {
      format: 'es'
    },
    server: {
      watch: {
        usePolling: true
      },
      proxy: {
        '/api': {
          // VITE_PROXY_TARGET for docker (http://api:8080), fallback to localhost for local dev
          target: env.VITE_PROXY_TARGET || 'http://localhost:6766',
          changeOrigin: true,
          ws: false
        }
      }
    }
  };
});
