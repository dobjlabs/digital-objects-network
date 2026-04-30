import { OBJECTS } from "../data";
import { invCount, type Inv } from "../sim";

interface Props {
  inv: Inv;
}

export function Stockpile({ inv }: Props) {
  const items = OBJECTS.filter(
    (o) => o.cat !== "level" && o.cat !== "station" && o.cat !== "tool",
  )
    .map((o) => ({ id: o.id, label: o.label, count: invCount(inv, o.id) }))
    .filter((e) => e.count > 0);

  return (
    <div>
      <div style={{ fontSize: 11, color: "#aaa", marginBottom: 4 }}>Stockpile</div>
      <div style={{ border: "1px solid #e8e8e8" }}>
        {items.length === 0 ? (
          <div style={{ padding: "4px 8px", fontSize: 11, color: "#ccc" }}>empty</div>
        ) : (
          items.map((e) => (
            <div
              key={e.id}
              style={{
                display: "flex",
                justifyContent: "space-between",
                padding: "3px 8px",
                borderBottom: "1px solid #f4f4f4",
                fontSize: 12,
              }}
            >
              <span style={{ color: "#777" }}>{e.label}</span>
              <span style={{ fontFamily: "monospace", fontWeight: "bold" }}>
                {e.count}
              </span>
            </div>
          ))
        )}
      </div>
    </div>
  );
}
