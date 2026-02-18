import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';

const apiTarget = process.env.VITE_API_URL || 'http://127.0.0.1:3001';
const zkArtifactsUrl = process.env.VITE_ZK_ARTIFACTS_URL || '';

export default defineConfig({
  plugins: [react()],
  resolve: {
    dedupe: ['react', 'react-dom']
  },
  define: {
    'import.meta.env.VITE_API_URL': JSON.stringify(apiTarget),
    'import.meta.env.VITE_ZK_ARTIFACTS_URL': JSON.stringify(zkArtifactsUrl)
  },
  server: {
    host: '0.0.0.0',
    port: 5000,
    allowedHosts: true,
    proxy: {
      '/api': {
        target: apiTarget,
        changeOrigin: true
      }
    }
  }
});
