import { afterEach, beforeEach, describe, expect, test } from "bun:test";

type Handler = (event: any, ctx: any) => Promise<any> | any;

class FakeAmp {
  handlers = new Map<string, Handler>();
  logger = { log: () => {} };
  system = {};
  configuration = {};
  helpers = {};
  ai = {};
  $ = async () => ({ stdout: "amp-session\n" });

  on(event: string, handler: Handler) {
    this.handlers.set(event, handler);
    return { unsubscribe: () => {} };
  }
}

describe("Amp opensessions plugin", () => {
  const previousFetch = globalThis.fetch;
  const previousTokenFile = process.env.OPENSESSIONS_TOKEN_FILE;
  const previousUrl = process.env.OPENSESSIONS_URL;
  const tokenFile = `/tmp/opensessions-plugin-test-${process.pid}.token`;

  beforeEach(async () => {
    await Bun.write(tokenFile, "test-token\n");
    process.env.OPENSESSIONS_TOKEN_FILE = tokenFile;
    process.env.OPENSESSIONS_URL = "http://127.0.0.1:1";
  });

  afterEach(() => {
    globalThis.fetch = previousFetch;
    if (previousTokenFile === undefined) {
      delete process.env.OPENSESSIONS_TOKEN_FILE;
    } else {
      process.env.OPENSESSIONS_TOKEN_FILE = previousTokenFile;
    }
    if (previousUrl === undefined) {
      delete process.env.OPENSESSIONS_URL;
    } else {
      process.env.OPENSESSIONS_URL = previousUrl;
    }
    try { require("fs").unlinkSync(tokenFile); } catch {}
  });

  test("keeps tool-running status until all in-flight tools finish", async () => {
    const posts: any[] = [];
    globalThis.fetch = (async (_input: string | URL | Request, init?: RequestInit) => {
      posts.push(JSON.parse(String(init?.body)));
      return new Response(null, { status: 204 });
    }) as typeof fetch;

    const { default: installPlugin } = await import("../../../integrations/amp/opensessions");
    const amp = new FakeAmp();
    installPlugin(amp as any);

    const ctx = { thread: { id: "T-plugin" }, $: amp.$ };
    await amp.handlers.get("session.start")!({ thread: { id: "T-plugin" } }, ctx);
    await amp.handlers.get("agent.start")!({ thread: { id: "T-plugin" }, message: "go", id: "m1" }, ctx);
    await amp.handlers.get("tool.call")!({ thread: { id: "T-plugin" }, toolUseID: "tool-1", tool: "Read", input: {} }, ctx);
    await amp.handlers.get("tool.call")!({ thread: { id: "T-plugin" }, toolUseID: "tool-2", tool: "Grep", input: {} }, ctx);
    await amp.handlers.get("tool.result")!({ thread: { id: "T-plugin" }, toolUseID: "tool-1", tool: "Read", input: {}, status: "done" }, ctx);
    await amp.handlers.get("tool.result")!({ thread: { id: "T-plugin" }, toolUseID: "tool-2", tool: "Grep", input: {}, status: "done" }, ctx);

    expect(posts.map((post) => post.status)).toEqual([
      "idle",
      "running",
      "tool-running",
      "tool-running",
      "tool-running",
      "running",
    ]);
    expect(posts.every((post) => post.threadId === "T-plugin")).toBe(true);
  });
});
