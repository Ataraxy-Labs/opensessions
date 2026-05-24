import { expect, test } from "bun:test";

import { PROTOCOL_VERSION, type ServerMessage } from "../src/shared";

test("ServerMessage supports the additive protocol hello", () => {
  const msg: ServerMessage = {
    type: "hello",
    protocol: PROTOCOL_VERSION,
    serverVersion: "0.2.0-alpha.5",
  };

  expect(msg).toEqual({
    type: "hello",
    protocol: 1,
    serverVersion: "0.2.0-alpha.5",
  });
});
