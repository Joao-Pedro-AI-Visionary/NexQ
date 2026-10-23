import { defineConfig } from 'astro/config';
import tailwind from '@astrojs/tailwind';
import sitemap from '@astrojs/sitemap';

// Deployed to GitHub Pages as a project site: https://naxhq.github.io/NexQ/
export default defineConfig({
  site: 'https://naxhq.github.io',
  base: '/NexQ/',
  output: 'static',
  integrations: [tailwind(), sitemap()],
});
