import { describe, test, expect } from "bun:test";
import type { MuxProvider, MuxProviderV1, MuxSessionInfo, FullMuxProvider } from "../src/contracts/mux";
import { isWindowCapable, isSidebarCapable, isBatchCapable, isFullSidebarCapable } from "../src/contracts/mux";

describe("Mux Contract", () => {
  test("MuxSessionInfo has required fields", () => {
    const info: MuxSessionInfo = {
      name: "my-session",
      createdAt: 1700000000,
      dir: "/home/user/project",
      windows: 2,
    };

    expect(info.name).toBe("my-session");
    expect(info.createdAt).toBe(1700000000);
    expect(info.dir).toBe("/home/user/project");
    expect(info.windows).toBe(2);
  });

  test("MuxProviderV1 interface has specificationVersion and required methods", () => {
    const mock: MuxProvider = {
      specificationVersion: "v1",
      name: "test-mux",
      listSessions: () => [],
      switchSession: (_name: string, _clientTty?: string) => {},
      getCurrentSession: () => null,
      getSessionDir: (_name: string) => "",
      getPaneCount: (_name: string) => 1,
      getClientTty: () => "",
      createSession: (_name?: string, _dir?: string) => {},
      killSession: (_name: string) => {},
      setupHooks: (_serverHost: string, _serverPort: number) => {},
      cleanupHooks: () => {},
    };

    expect(mock.specificationVersion).toBe("v1");
    expect(mock.name).toBe("test-mux");
    expect(mock.listSessions()).toEqual([]);
    expect(mock.getCurrentSession()).toBeNull();
    expect(mock.getPaneCount("test")).toBe(1);
    expect(mock.getClientTty()).toBe("");
  });

  test("MuxProvider supports optional capability methods", () => {
    const mock: MuxProvider = {
      specificationVersion: "v1",
      name: "test-mux",
      listSessions: () => [],
      switchSession: () => {},
      getCurrentSession: () => null,
      getSessionDir: () => "",
      getPaneCount: () => 1,
      getClientTty: () => "",
      createSession: () => {},
      killSession: () => {},
      setupHooks: () => {},
      cleanupHooks: () => {},
      listSidebarPanes: () => [],
      spawnSidebar: () => null,
      hideSidebar: () => {},
      killSidebarPane: () => {},
      resizeSidebarPane: () => {},
      killOrphanedSidebarPanes: () => {},
      killStaleSidebarPanes: () => {},
      cleanupSidebar: () => {},
      listActiveWindows: () => [],
      getCurrentWindowId: () => null,
      getAllPaneCounts: () => new Map(),
    };
    expect(mock.listSidebarPanes!()).toEqual([]);
    expect(mock.listActiveWindows!()).toEqual([]);
  });

  test("type guards correctly narrow capabilities", () => {
    // Minimal provider — no capabilities
    const minimal: MuxProvider = {
      specificationVersion: "v1",
      name: "minimal",
      listSessions: () => [],
      switchSession: () => {},
      getCurrentSession: () => null,
      getSessionDir: () => "",
      getPaneCount: () => 1,
      getClientTty: () => "",
      createSession: () => {},
      killSession: () => {},
      setupHooks: () => {},
      cleanupHooks: () => {},
    };

    expect(isWindowCapable(minimal)).toBe(false);
    expect(isSidebarCapable(minimal)).toBe(false);
    expect(isBatchCapable(minimal)).toBe(false);
    expect(isFullSidebarCapable(minimal)).toBe(false);

    // Full provider — all capabilities
    const full: MuxProvider = {
      ...minimal,
      listActiveWindows: () => [],
      getCurrentWindowId: () => null,
      listSidebarPanes: () => [],
      spawnSidebar: () => null,
      hideSidebar: () => {},
      killSidebarPane: () => {},
      resizeSidebarPane: () => {},
      killOrphanedSidebarPanes: () => {},
      killStaleSidebarPanes: () => {},
      cleanupSidebar: () => {},
      getAllPaneCounts: () => new Map(),
    };

    expect(isWindowCapable(full)).toBe(true);
    expect(isSidebarCapable(full)).toBe(true);
    expect(isBatchCapable(full)).toBe(true);
    expect(isFullSidebarCapable(full)).toBe(true);
  });
});
