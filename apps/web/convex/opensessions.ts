import { mutation, query } from "./_generated/server";
import { v } from "convex/values";

export const publishSnapshot = mutation({
  args: {
    apiKey: v.string(),
    snapshot: v.any(),
  },
  handler: async (ctx, { apiKey, snapshot }) => {
    const nodeId = snapshot?.nodeId;
    if (typeof nodeId !== "string" || nodeId.length === 0) {
      throw new Error("snapshot.nodeId is required");
    }

    const now = Date.now();
    const existing = await ctx.db
      .query("opensessionsSnapshots")
      .withIndex("by_key", (q) => q.eq("apiKey", apiKey).eq("nodeId", nodeId))
      .unique();

    const doc = {
      apiKey,
      nodeId,
      snapshot,
      updatedAt: now,
    };

    if (existing) {
      await ctx.db.patch(existing._id, doc);
    } else {
      await ctx.db.insert("opensessionsSnapshots", doc);
    }

    const uiRevision = Number(snapshot?.uiState?.ts ?? 0);
    if (snapshot?.uiState && uiRevision > 0) {
      const existingUi = await ctx.db
        .query("opensessionsUiState")
        .withIndex("by_account", (q) => q.eq("apiKey", apiKey))
        .unique();
      if (!existingUi || uiRevision > existingUi.revision) {
        const uiDoc = {
          apiKey,
          state: snapshot.uiState,
          revision: uiRevision,
          updatedAt: now,
        };
        if (existingUi) {
          await ctx.db.patch(existingUi._id, uiDoc);
        } else {
          await ctx.db.insert("opensessionsUiState", uiDoc);
        }
      }
    }
  },
});

export const getGraph = query({
  args: { apiKey: v.string() },
  handler: async (ctx, { apiKey }) => {
    const rows = await ctx.db
      .query("opensessionsSnapshots")
      .withIndex("by_account", (q) => q.eq("apiKey", apiKey))
      .collect();

    const commandIntentRows = await ctx.db
      .query("opensessionsCommandIntents")
      .withIndex("by_account", (q) => q.eq("apiKey", apiKey))
      .filter((q) => q.eq(q.field("status"), "pending"))
      .collect();
    const commandIntents = commandIntentRows.map((row) => ({
      id: row.intentId,
      action: row.action,
      nodeId: row.targetNodeId,
      providerId: row.providerId,
      session: row.session,
      payload: row.payload,
      ts: row.createdAt,
    }));
    const uiState = await ctx.db
      .query("opensessionsUiState")
      .withIndex("by_account", (q) => q.eq("apiKey", apiKey))
      .unique();

    for (const row of rows) {
      const snapshot = row.snapshot as any;
      if (Array.isArray(snapshot?.commandIntents)) {
        commandIntents.push(...snapshot.commandIntents);
      }
    }

    return {
      nodes: rows.map((row) => row.snapshot),
      uiState: uiState?.state ?? null,
      commandIntents,
    };
  },
});

export const enqueueCommandIntent = mutation({
  args: {
    apiKey: v.string(),
    intent: v.any(),
  },
  handler: async (ctx, { apiKey, intent }) => {
    const intentId = intent?.id;
    const targetNodeId = intent?.nodeId;
    const action = intent?.action;
    if (typeof intentId !== "string" || intentId.length === 0) {
      throw new Error("intent.id is required");
    }
    if (typeof targetNodeId !== "string" || targetNodeId.length === 0) {
      throw new Error("intent.nodeId is required");
    }
    if (typeof action !== "string" || action.length === 0) {
      throw new Error("intent.action is required");
    }

    const now = Date.now();
    const existing = await ctx.db
      .query("opensessionsCommandIntents")
      .withIndex("by_intent", (q) => q.eq("apiKey", apiKey).eq("intentId", intentId))
      .unique();
    const doc = {
      apiKey,
      intentId,
      targetNodeId,
      action,
      providerId: typeof intent?.providerId === "string" ? intent.providerId : undefined,
      session: typeof intent?.session === "string" ? intent.session : undefined,
      payload: intent?.payload ?? {},
      status: "pending" as const,
      result: undefined,
      createdAt: Number(intent?.ts ?? now),
      updatedAt: now,
    };
    if (existing) {
      await ctx.db.patch(existing._id, doc);
    } else {
      await ctx.db.insert("opensessionsCommandIntents", doc);
    }
  },
});

export const completeCommandIntent = mutation({
  args: {
    apiKey: v.string(),
    intentId: v.string(),
    status: v.union(v.literal("completed"), v.literal("failed")),
    result: v.optional(v.string()),
  },
  handler: async (ctx, { apiKey, intentId, status, result }) => {
    const existing = await ctx.db
      .query("opensessionsCommandIntents")
      .withIndex("by_intent", (q) => q.eq("apiKey", apiKey).eq("intentId", intentId))
      .unique();
    if (existing) {
      await ctx.db.patch(existing._id, {
        status,
        result,
        updatedAt: Date.now(),
      });
    }
  },
});

export const deleteSnapshot = mutation({
  args: {
    apiKey: v.string(),
    nodeId: v.string(),
  },
  handler: async (ctx, { apiKey, nodeId }) => {
    const existing = await ctx.db
      .query("opensessionsSnapshots")
      .withIndex("by_key", (q) => q.eq("apiKey", apiKey).eq("nodeId", nodeId))
      .unique();
    if (existing) {
      await ctx.db.delete(existing._id);
    }
  },
});
