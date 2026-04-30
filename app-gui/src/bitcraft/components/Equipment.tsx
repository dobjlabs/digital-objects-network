import { label, LEVEL_IDS, STATION_IDS, TOOL_IDS } from "../data";
import { invBusy, invCount, usesAvailable, type Inv } from "../sim";

interface Props {
  inv: Inv;
  busy: Record<string, boolean>;
}

export function Equipment({ inv, busy }: Props) {
  const levels = LEVEL_IDS.filter((id) => invCount(inv, id) > 0);
  const stations = STATION_IDS.filter(
    (id) => invCount(inv, id) > 0 || invBusy(inv, id) || busy[id],
  );
  const tools = TOOL_IDS.filter((id) => invCount(inv, id) > 0);

  if (!levels.length && !stations.length && !tools.length) return null;

  const row = (id: string, statusText: string, color: string) => (
    <div
      key={id}
      style={{
        display: "flex",
        justifyContent: "space-between",
        alignItems: "center",
        padding: "3px 8px",
        borderBottom: "1px solid #f4f4f4",
        fontSize: 12,
      }}
    >
      <span style={{ color: "#777" }}>{label(id)}</span>
      <span style={{ fontSize: 10, color, fontFamily: "monospace" }}>
        {statusText}
      </span>
    </div>
  );

  return (
    <div>
      <div style={{ fontSize: 11, color: "#aaa", marginBottom: 4 }}>Equipment</div>
      <div style={{ border: "1px solid #e8e8e8" }}>
        {levels.map((id) => row(id, "active", "#aaa"))}
        {stations.map((id) => {
          const isBusy = invBusy(inv, id) || busy[id] === true;
          return row(id, isBusy ? "busy" : "free", isBusy ? "#c00" : "#4a9");
        })}
        {tools.map((id) => {
          const uses = usesAvailable(inv, id);
          return row(
            id,
            `${uses} use${uses !== 1 ? "s" : ""} left`,
            uses <= 1 ? "#c00" : "#888",
          );
        })}
      </div>
    </div>
  );
}
