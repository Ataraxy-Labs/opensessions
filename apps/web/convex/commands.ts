import { mutation, query } from "./_generated/server";
import { v } from "convex/values";

// Web/phone inserts a pending command. The target machine's bridge picks it up.
export const enqueue = mutation({
  args: {
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
    payload: v.optional(v.string()),
  },
  handler: async (ctx, args) =>
    await ctx.db.insert("commands", {
      ...args,
      status: "pending",
      createdAt: Date.now(),
    }),
});

// The bridge subscribes to this — its inbox of undelivered work.
export const inbox = query({
  args: { account: v.string(), machineId: v.string() },
  handler: async (ctx, { account, machineId }) =>
    await ctx.db
      .query("commands")
      .withIndex("by_inbox", (q) =>
        q.eq("account", account).eq("machineId", machineId).eq("status", "pending"),
      )
      .collect(),
});

// Bridge flips status as it claims / finishes a command.
export const update = mutation({
  args: {
    id: v.id("commands"),
    status: v.union(
      v.literal("delivered"),
      v.literal("acked"),
      v.literal("failed"),
    ),
    result: v.optional(v.string()),
  },
  handler: async (ctx, { id, status, result }) =>
    await ctx.db.patch(id, { status, result }),
});
