import type { FeedPost, InventoryItem, Recipe } from "../types/domain";

export const mockItems: InventoryItem[] = [
  // Intentionally empty for now; these should come from real local files next.
];

export const mockRecipes: Recipe[] = [
  {
    id: "recipe-log",
    name: "Log",
    emoji: "🪵",
    verb: "Mine",
    desc: "Mine a Log. No inputs required.",
    cpu: "20-40s",
    readsBlock: false,
    consumes: [],
    requires: [],
    unlocked: true,
  },
];

export const mockFeed: FeedPost[] = [
  {
    id: "post-asteroid-party",
    title: "Asteroid #14 at 84% charge - mining party tonight",
    peer: "192.168.1.14",
    time: "Mar 1 21:04",
    desc: "Need at least 3 pickaxes tier 1+. Crystal yield split equally.",
    proofs: [{ name: "Asteroid", validity: "live", hash: "0x1a2b...c3d4" }],
    responses: [],
  },
  {
    id: "post-wtb-dragongem",
    title: "WTB Dragon Gem - offering CoinPurse x3",
    peer: "10.0.0.7",
    time: "Mar 1 20:47",
    desc: "Trading with live proofs only.",
    proofs: [
      { name: "CoinPurse", validity: "live", hash: "0xc5c6...1e2f" },
      { name: "CoinPurse", validity: "nullified", hash: "0xd6d7...2f30" },
    ],
    responses: [],
  },
];
