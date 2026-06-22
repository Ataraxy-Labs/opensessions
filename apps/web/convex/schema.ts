import { defineSchema, defineTable } from "convex/server";
import { v } from "convex/values";

// Mirrors runtime-rs `AgentStatus` (serde camelCase). One shape for every agent.
export const agentStatus = v.union(
  v.literal("idle"),
  v.literal("running"),
  v.literal("toolRunning"),
  v.literal("done"),
  v.literal("error"),
  v.literal("waiting"),
  v.literal("interrupted"),
  v.literal("stale"),
);

export default defineSchema({
  // One row per machine (laptop, ssh box). Heartbeated by ingestSnapshot.
  machines: defineTable({
    account: v.string(),
    machineId: v.string(),
    hostname: v.string(),
    lastSeen: v.number(),
  })
    .index("by_account", ["account"])
    .index("by_machine", ["account", "machineId"]),

  // One row per (machine, agent, thread). Mirrors AgentWatcherSnapshot.
  agents: defineTable({
    account: v.string(),
    machineId: v.string(),
    hostname: v.string(),
    agent: v.string(), // "claude-code" | "codex" | "opencode" | "pi" | "amp" | "droid"
    threadId: v.string(), // "" when the watcher has none
    threadName: v.optional(v.string()),
    lastUserPrompt: v.optional(v.string()),
    projectDir: v.optional(v.string()),
    paneId: v.optional(v.string()),
    status: agentStatus,
    ts: v.number(), // snapshot time, from the watcher
    updatedAt: v.number(), // cloud receive time
  })
    .index("by_account", ["account"])
    .index("by_key", ["account", "machineId", "agent", "threadId"]),

  // Command bus. Web inserts `pending`; the per-machine bridge drains its inbox.
  commands: defineTable({
    account: v.string(),
    machineId: v.string(),
    kind: v.union(
      v.literal("sendInput"),
      v.literal("focusPane"),
      v.literal("kill"),
    ),
    target: v.object({
      paneId: v.optional(v.string()),
      threadId: v.optional(v.string()),
      agent: v.optional(v.string()),
    }),
    payload: v.optional(v.string()), // text for sendInput
    status: v.union(
      v.literal("pending"),
      v.literal("delivered"),
      v.literal("acked"),
      v.literal("failed"),
    ),
    result: v.optional(v.string()),
    createdAt: v.number(),
  }).index("by_inbox", ["account", "machineId", "status"]),

  // Temporary hosted opensessions workspace relay. Each opensessions server
  // publishes one full node snapshot; readers materialize a global graph.
  opensessionsSnapshots: defineTable({
    apiKey: v.string(),
    nodeId: v.string(),
    snapshot: v.any(),
    updatedAt: v.number(),
  })
    .index("by_account", ["apiKey"])
    .index("by_key", ["apiKey", "nodeId"]),

  opensessionsUiState: defineTable({
    apiKey: v.string(),
    state: v.any(),
    revision: v.number(),
    updatedAt: v.number(),
  }).index("by_account", ["apiKey"]),

  opensessionsCommandIntents: defineTable({
    apiKey: v.string(),
    intentId: v.string(),
    targetNodeId: v.string(),
    action: v.string(),
    providerId: v.optional(v.string()),
    session: v.optional(v.string()),
    payload: v.any(),
    status: v.union(
      v.literal("pending"),
      v.literal("completed"),
      v.literal("failed"),
    ),
    result: v.optional(v.string()),
    createdAt: v.number(),
    updatedAt: v.number(),
  })
    .index("by_account", ["apiKey"])
    .index("by_inbox", ["apiKey", "targetNodeId", "status"])
    .index("by_intent", ["apiKey", "intentId"]),
});
