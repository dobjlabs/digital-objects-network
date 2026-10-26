// Presentation packs: app-side icons and text for plugin classes and actions.
//
// A pack is a plugin's `pack.json` plus the assets it names. An app ships
// the packs it wants under one folder, with an `index.json` listing them:
//
//   presentation/index.json         { "packs": ["bitcraft/pack.json"] }
//   presentation/bitcraft/pack.json
//   presentation/bitcraft/assets/...
//
// pack.json:
//
//   {
//     "format": 1,
//     "plugin": "bitcraft",
//     "writtenFor": { "moduleHash": "..." },
//     "classes": { "Wood": { "icon": "assets/...", "summary": "...", "description": "..." } },
//     "actions": { "GatherWood": { "icon": "assets/...", "summary": "...", "description": "..." } }
//   }
//
// Every field is optional. Icon paths are relative to the pack's folder and
// name finished images, so an app only displays them. Packs are optional
// too: a missing, unreadable or malformed pack, or a missing entry or field,
// leaves that information empty rather than failing. Nothing here talks to
// dobjd, and dobjd knows nothing of packs.
//
// This file has no dependencies and no build step; copy it into another app
// as is. See apps/bitcraft/PRESENTATION_PACKS.md.

const Presentation = (() => {
  // Each loaded pack, by plugin name: its parsed JSON and the URL it was
  // read from, which its asset paths resolve against.
  const packs = new Map();

  // Reads the index and every pack it lists, and resolves once all have been
  // tried. Never rejects.
  async function load(indexPath = "presentation/index.json") {
    packs.clear();
    const indexUrl = new URL(indexPath, document.baseURI);
    const index = await readJson(indexUrl);
    const paths = Array.isArray(index?.packs) ? index.packs : [];
    await Promise.all(paths.map(async (path) => {
      if (typeof path !== "string") return;
      const packUrl = new URL(path, indexUrl);
      const pack = await readJson(packUrl);
      if (pack && typeof pack.plugin === "string") {
        packs.set(pack.plugin, { pack, url: packUrl });
      }
    }));
  }

  async function readJson(url) {
    try {
      const response = await fetch(url.href);
      return response.ok ? await response.json() : null;
    } catch {
      return null;
    }
  }

  // What the packs say about a class or action, named by its qualified name
  // ({ pluginName, name }): an absolute icon URL, a summary and a
  // description, each null when unknown.
  function lookup(table, qualifiedName) {
    const loaded = packs.get(qualifiedName?.pluginName);
    const entry = loaded?.pack?.[table]?.[qualifiedName?.name];
    const text = (value) => (typeof value === "string" && value !== "" ? value : null);
    let icon = null;
    if (typeof entry?.icon === "string" && entry.icon !== "") {
      try {
        icon = new URL(entry.icon, loaded.url).href;
      } catch {
        icon = null;
      }
    }
    return { icon, summary: text(entry?.summary), description: text(entry?.description) };
  }

  return {
    load,
    classInfo: (qualifiedName) => lookup("classes", qualifiedName),
    actionInfo: (qualifiedName) => lookup("actions", qualifiedName),
  };
})();
