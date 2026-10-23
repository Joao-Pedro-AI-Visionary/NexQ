/** @type {import('tailwindcss').Config} */
export default {
  content: ['./src/**/*.{astro,html,js,ts,md}'],
  theme: {
    extend: {
      colors: {
        // Mirrors the tokens in src/styles/global.css (hex so opacity modifiers work).
        // Warm slate-blue neutrals. Every accent carries meaning:
        // blue = primary action / brand, green = live / success,
        // amber = the other party / caution, red = recording / destructive.
        bg: {
          DEFAULT: '#0b0d13',
          raised: '#11141c',
          sunken: '#08090e',
          panel: '#161a24',
        },
        ink: {
          DEFAULT: '#eef1f7',
          2: '#a3aabd',
          3: '#838aa0',
        },
        line: {
          DEFAULT: 'rgba(160, 175, 215, 0.1)',
          strong: 'rgba(160, 175, 215, 0.18)',
        },
        brand: {
          DEFAULT: '#3b82f6',
          ink: '#7aa7ff',
          btn: '#2563eb',
        },
        live: '#3ddc97',
        them: '#f2b84b',
        rec: '#f26d6d',
      },
      fontFamily: {
        sans: ['Geist', 'ui-sans-serif', 'system-ui', 'Segoe UI', 'sans-serif'],
        mono: ['"Geist Mono"', 'ui-monospace', '"Cascadia Code"', 'Menlo', 'monospace'],
        serif: ['"Instrument Serif"', 'ui-serif', 'Georgia', 'serif'],
      },
      maxWidth: {
        page: '72rem',
      },
      transitionTimingFunction: {
        // Critically damped spring approximation: fast start, no overshoot.
        spring: 'cubic-bezier(0.22, 1, 0.36, 1)',
      },
    },
  },
  plugins: [],
};
