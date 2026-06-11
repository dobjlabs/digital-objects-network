import { defineConfig } from 'vocs/config'

// Injected by the Pages deploy workflow ("/<repo>" on <owner>.github.io);
// unset for local dev and custom domains.
const basePath = process.env.BASE_PATH || undefined

export default defineConfig({
  basePath,
  // vocs cannot resolve an anchor on the home page (index route) to a file, so a
  // relative link like (../#public-infrastructure) is flagged as a false-positive
  // deadlink even though it renders correctly. Warn instead of failing the build.
  checkDeadlinks: 'warn',
  title: 'Digital Objects Network',
  description:
    'Privately-held, fully programmable stateful objects, exchanged between mutually untrusting users and anchored to Ethereum blob data availability.',
  accentColor: 'light-dark(#0e7490, #22d3ee)',
  banner: {
    dismissable: true,
    backgroundColor: '#0e7490',
    textColor: 'white',
    content: `Testnet on Ethereum Sepolia - [install the driver](${basePath ?? ''}/install) to join.`,
  },
  topNav: [
    { text: 'Network', link: '/network' },
    { text: 'Install', link: '/install' },
    { text: 'Applications', link: '/applications' },
    { text: 'GitHub', link: 'https://github.com/dobjlabs/digital-objects-network' },
  ],
  socials: [
    { icon: 'github', link: 'https://github.com/dobjlabs/digital-objects-network' },
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
    link: 'https://github.com/dobjlabs/digital-objects-network/edit/main/docs/src/pages/:path',
  },
})
