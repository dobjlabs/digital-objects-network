import { defineConfig } from 'vocs'

// Injected by the Pages deploy workflow ("/<repo>" on <owner>.github.io);
// unset for local dev and custom domains.
const basePath = process.env.BASE_PATH || undefined

export default defineConfig({
  rootDir: '.',
  basePath,
  title: 'Digital Objects Network',
  description:
    'Privately-held, fully programmable stateful objects, exchanged between mutually untrusting users and anchored to Ethereum blob data availability.',
  banner: {
    dismissable: true,
    content:
      `Testnet on Ethereum Sepolia - [install the driver](${basePath ?? ''}/install) to join.`,
  },
  theme: {
    accentColor: {
      light: '#0e7490',
      dark: '#22d3ee',
    },
  },
  topNav: [
    { text: 'Network', link: '/network' },
    { text: 'Install', link: '/install' },
    { text: 'Applications', link: '/applications' },
    { text: 'GitHub', link: 'https://github.com/dobjlabs/digital-objects-network' },
  ],
  sidebar: [
    {
      text: 'Network',
      items: [
        { text: 'Digital Objects Network', link: '/network' },
        { text: 'Architecture', link: '/architecture' },
      ],
    },
    {
      text: 'Getting started',
      items: [{ text: 'Install the driver', link: '/install' }],
    },
    {
      text: 'Applications',
      items: [
        { text: 'Plugins and how to install', link: '/applications' },
        { text: 'craft-basics', link: '/applications/craft-basics' },
        { text: 'craft-rocket', link: '/applications/craft-rocket' },
      ],
    },
  ],
  editLink: {
    pattern:
      'https://github.com/dobjlabs/digital-objects-network/edit/main/docs/pages/:path',
    text: 'Suggest changes to this page',
  },
})
