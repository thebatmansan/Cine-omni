import { describe, expect, it } from "vitest";
import { RpcClient } from "./client";

describe("RpcClient", () => {
  it("encodes JSON-RPC 2.0 requests with increasing ids", () => {
    const c = new RpcClient("ws://localhost:0");
    const a = c.encode("stats");
    const b = c.encode("edit_dialogue", { line: "x", text: "y" });
    expect(a.id).toBe(1);
    expect(b.id).toBe(2);
    expect(JSON.parse(a.line)).toEqual({ jsonrpc: "2.0", id: 1, method: "stats", params: null });
    expect(JSON.parse(b.line).params).toEqual({ line: "x", text: "y" });
  });

  it("dispatches change notifications to listeners and ignores garbage", () => {
    const c = new RpcClient("ws://localhost:0");
    const seen: number[] = [];
    c.onChange((n) => seen.push(n.revision));
    c.onMessage(JSON.stringify({ jsonrpc: "2.0", method: "changed", params: { revision: 7, label: "x", depth: 0, touched: [], dirty: 1 } }));
    c.onMessage("not json");
    c.onMessage(JSON.stringify({ jsonrpc: "2.0", id: 999, result: 1 }));
    expect(seen).toEqual([7]);
  });
});
