import { useState } from "react";
import { useQuery, useMutation } from "convex/react";
import { api } from "../convex/_generated/api";

// Placeholder identity until real auth — every machine + this UI share it.
const ACCOUNT = "dev";

const STATUS_COLOR: Record<string, string> = {
  idle: "#6b7280",
  running: "#3b82f6",
  toolRunning: "#8b5cf6",
  waiting: "#f59e0b",
  done: "#10b981",
  error: "#ef4444",
  interrupted: "#9ca3af",
  stale: "#b45309",
};

export default function App() {
  const agents = useQuery(api.agents.listAgents, { account: ACCOUNT });
  const machines = useQuery(api.machines.listMachines, { account: ACCOUNT });
  const enqueue = useMutation(api.commands.enqueue);

  if (agents === undefined || machines === undefined) {
    return <main style={{ padding: 24, fontFamily: "system-ui" }}>Connecting…</main>;
  }

  const byMachine = new Map<string, typeof agents>();
  for (const a of agents) {
    const list = byMachine.get(a.machineId) ?? [];
    list.push(a);
    byMachine.set(a.machineId, list);
  }

  return (
    <main style={{ padding: 24, fontFamily: "system-ui", maxWidth: 760, margin: "0 auto" }}>
      <h1 style={{ fontSize: 20 }}>opensessions · fleet</h1>
      {machines.length === 0 && <p>No machines yet. Start a bridge to see agents here.</p>}
      {machines.map((m) => (
        <section key={m.machineId} style={{ marginTop: 20 }}>
          <h2 style={{ fontSize: 14, color: "#374151" }}>
            {m.hostname}{" "}
            <span style={{ color: "#9ca3af", fontWeight: 400 }}>· {m.machineId}</span>
          </h2>
          {(byMachine.get(m.machineId) ?? []).map((a) => (
            <AgentCard
              key={a._id}
              agent={a}
              onSend={(text) =>
                enqueue({
                  account: ACCOUNT,
                  machineId: a.machineId,
                  kind: "sendInput",
                  target: { paneId: a.paneId, threadId: a.threadId, agent: a.agent },
                  payload: text,
                })
              }
            />
          ))}
        </section>
      ))}
    </main>
  );
}

function AgentCard({
  agent,
  onSend,
}: {
  agent: any;
  onSend: (text: string) => void;
}) {
  const [text, setText] = useState("");
  return (
    <div
      style={{
        border: "1px solid #e5e7eb",
        borderRadius: 8,
        padding: 12,
        marginTop: 8,
      }}
    >
      <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
        <span
          style={{
            width: 8,
            height: 8,
            borderRadius: 99,
            background: STATUS_COLOR[agent.status] ?? "#6b7280",
          }}
        />
        <strong>{agent.agent}</strong>
        <span style={{ color: "#6b7280", fontSize: 13 }}>{agent.status}</span>
        {agent.projectDir && (
          <span style={{ color: "#9ca3af", fontSize: 12, marginLeft: "auto" }}>
            {agent.projectDir}
          </span>
        )}
      </div>
      {agent.threadName && (
        <div style={{ marginTop: 6, fontSize: 14 }}>{agent.threadName}</div>
      )}
      {agent.lastUserPrompt && (
        <div style={{ marginTop: 4, fontSize: 13, color: "#6b7280" }}>
          {agent.lastUserPrompt}
        </div>
      )}
      <form
        onSubmit={(e) => {
          e.preventDefault();
          if (!text.trim()) return;
          onSend(text);
          setText("");
        }}
        style={{ marginTop: 8, display: "flex", gap: 6 }}
      >
        <input
          value={text}
          onChange={(e) => setText(e.target.value)}
          placeholder="reply to this agent…"
          style={{ flex: 1, padding: "6px 8px", borderRadius: 6, border: "1px solid #d1d5db" }}
        />
        <button type="submit" style={{ padding: "6px 12px", borderRadius: 6 }}>
          Send
        </button>
      </form>
    </div>
  );
}
