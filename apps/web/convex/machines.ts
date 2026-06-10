import { query } from "./_generated/server";
import { v } from "convex/values";

export const listMachines = query({
  args: { account: v.string() },
  handler: async (ctx, { account }) =>
    await ctx.db
      .query("machines")
      .withIndex("by_account", (q) => q.eq("account", account))
      .collect(),
});
