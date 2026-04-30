import { useMemo } from "react";

const STUB = [
  { name: "satoshi.eth", score: 47 },
  { name: "0x3f4a...8c2b", score: 31 },
  { name: "vitalik.eth", score: 28 },
  { name: "0x9b2e...4f1a", score: 19 },
  { name: "alice.eth", score: 14 },
  { name: "0xc7d1...3a9e", score: 9 },
  { name: "0x2f8b...7c4d", score: 7 },
  { name: "0x5e3a...1b8f", score: 4 },
  { name: "0x8d2c...6e5b", score: 2 },
  { name: "0x1a4f...9c7e", score: 1 },
];

interface Props {
  presentedScore: number | null;
}

export function Leaderboard({ presentedScore }: Props) {
  const rows = useMemo(() => {
    const list: Array<{ name: string; score: number; isYou?: boolean }> = [...STUB];
    if (presentedScore !== null) {
      const myRow = { name: "you", score: presentedScore, isYou: true };
      const idx = list.findIndex((r) => r.score <= presentedScore);
      if (idx === -1) list.push(myRow);
      else list.splice(idx, 0, myRow);
    }
    return list.slice(0, 12).map((r, i) => ({ ...r, rank: i + 1 }));
  }, [presentedScore]);

  return (
    <table
      style={{
        width: "100%",
        borderCollapse: "collapse",
        fontSize: 11,
        fontFamily: "system-ui,sans-serif",
      }}
    >
      <thead>
        <tr style={{ borderBottom: "1px solid #e8e8e8" }}>
          <td style={{ padding: "2px 4px", color: "#bbb", fontSize: 10 }}>#</td>
          <td style={{ padding: "2px 4px", color: "#bbb", fontSize: 10 }}>player</td>
          <td
            style={{
              padding: "2px 4px",
              color: "#bbb",
              fontSize: 10,
              textAlign: "right",
            }}
          >
            rockets
          </td>
        </tr>
      </thead>
      <tbody>
        {rows.map((r, i) => (
          <tr
            key={i}
            style={{
              background: r.isYou ? "#f5f5f5" : "transparent",
              fontWeight: r.isYou ? "bold" : "normal",
            }}
          >
            <td style={{ padding: "1px 4px", color: "#ccc", fontSize: 10 }}>
              {r.rank}
            </td>
            <td
              style={{
                padding: "1px 4px",
                overflow: "hidden",
                textOverflow: "ellipsis",
                whiteSpace: "nowrap",
                maxWidth: 120,
              }}
            >
              {r.name}
            </td>
            <td
              style={{
                padding: "1px 4px",
                textAlign: "right",
                fontFamily: "monospace",
              }}
            >
              {r.score}
            </td>
          </tr>
        ))}
      </tbody>
    </table>
  );
}
