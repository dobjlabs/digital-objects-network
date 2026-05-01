import { defineConfig } from "vocs";

export default defineConfig({
  title: "zk-craft",
  titleTemplate: "%s · zk-craft",
  description:
    "A network for stateful, verifiable, privately-held objects on the open Internet — anchored to Ethereum blob data availability, with rules enforced by ZK proofs.",
  rootDir: ".",
  basePath: "/zk-craft",
  theme: {
    accentColor: "#f59e0b",
  },
  topNav: [
    { text: "Overview", link: "/", match: "/" },
    { text: "Objects", link: "/objects", match: "/objects" },
    { text: "Actions", link: "/actions", match: "/actions" },
    { text: "Driver", link: "/driver", match: "/driver" },
    { text: "Synchronizer", link: "/synchronizer", match: "/synchronizer" },
    { text: "ZK Stack", link: "/zk-stack", match: "/zk-stack" },
    { text: "Glossary", link: "/glossary", match: "/glossary" },
    {
      text: "GitHub",
      link: "https://github.com/dobjlabs/zk-craft",
    },
  ],
  socials: [
    {
      icon: "github",
      link: "https://github.com/dobjlabs/zk-craft",
    },
  ],
  sidebar: [
    {
      text: "Overview",
      link: "/",
    },
    {
      text: "Glossary",
      link: "/glossary",
    },
    {
      text: "Objects",
      collapsed: false,
      items: [
        { text: "What is a digital object?", link: "/objects" },
        { text: "File structure", link: "/objects/structure" },
        { text: "Lifecycle & validity", link: "/objects/lifecycle" },
        { text: "Network & privacy model", link: "/objects/network" },
      ],
    },
    {
      text: "Actions & Pexes",
      collapsed: false,
      items: [
        { text: "Actions, classes, and pexes", link: "/actions" },
        {
          text: "Transactions (the state machine)",
          link: "/actions/transactions",
        },
        { text: "The .pexe archive format", link: "/actions/pexe-format" },
        { text: "Writing an action (SDK)", link: "/actions/sdk" },
      ],
    },
    {
      text: "Driver",
      collapsed: false,
      items: [
        { text: "What the driver does", link: "/driver" },
        { text: "Local store layout", link: "/driver/local-store" },
        { text: "API surface", link: "/driver/api" },
        { text: "Action execution flow", link: "/driver/execution" },
      ],
    },
    {
      text: "Synchronizer",
      collapsed: false,
      items: [
        { text: "What the synchronizer does", link: "/synchronizer" },
        { text: "Slot derivation", link: "/synchronizer/slot-derivation" },
        { text: "Storage model", link: "/synchronizer/storage" },
        { text: "HTTP API", link: "/synchronizer/api" },
      ],
    },
    {
      text: "ZK Stack",
      collapsed: false,
      items: [
        { text: "Overview", link: "/zk-stack" },
        { text: "Plonky2", link: "/zk-stack/plonky2" },
        { text: "pod2", link: "/zk-stack/pod2" },
        { text: "MainPod & shrinking", link: "/zk-stack/mainpod" },
      ],
    },
  ],
  editLink: {
    pattern: "https://github.com/dobjlabs/zk-craft/edit/main/docs/pages/:path",
    text: "Edit this page on GitHub",
  },
});
