import { mutation, query } from "./_generated/server";
import { v } from "convex/values";
import { agentStatus } from "./schema";

// Called by the per-machine bridge for every AgentWatcherSnapshot.
// Upserts the agent row and heartbeats the machine in one shot.
export const ingestSnapshot = mutation({
  args: {
    account: v.string(),
    machineId: v.string(),
    hostname: v.string(),
    agent: v.string(),
    threadId: v.optional(v.string()),
    threadName: v.optional(v.string()),
    lastUserPrompt: v.optional(v.string()),
    projectDir: v.optional(v.string()),
    paneId: v.optional(v.string()),
    status: agentStatus,
    ts: v.number(),
  },
  handler: async (ctx, args) => {
    const now = Date.now();
    const threadId = args.threadId ?? "";

    const existing = await ctx.db
      .query("agents")
      .withIndex("by_key", (q) =>
        q
          .eq("account", args.account)
          .eq("machineId", args.machineId)
          .eq("agent", args.agent)
          .eq("threadId", threadId),
      )
      .unique();

    const doc = {
      account: args.account,
      machineId: args.machineId,
      hostname: args.hostname,
      agent: args.agent,
      threadId,
      threadName: args.threadName,
      lastUserPrompt: args.lastUserPrompt,
      projectDir: args.projectDir,
      paneId: args.paneId,
      status: args.status,
      ts: args.ts,
      updatedAt: now,
    };

    if (existing) {
      await ctx.db.patch(existing._id, doc);
    } else {
      await ctx.db.insert("agents", doc);
    }

    const machine = await ctx.db
      .query("machines")
      .withIndex("by_machine", (q) =>
        q.eq("account", args.account).eq("machineId", args.machineId),
      )
      .unique();
    if (machine) {
      await ctx.db.patch(machine._id, { lastSeen: now, hostname: args.hostname });
    } else {
      await ctx.db.insert("machines", {
        account: args.account,
        machineId: args.machineId,
        hostname: args.hostname,
        lastSeen: now,
      });
    }
  },
});

// The global sidebar: every agent across every machine on the account.
export const listAgents = query({
  args: { account: v.string() },
  handler: async (ctx, { account }) =>
    await ctx.db
      .query("agents")
      .withIndex("by_account", (q) => q.eq("account", account))
      .collect(),
});
