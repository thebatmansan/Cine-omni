// WebSocket ⇄ stdio bridge: each browser client gets one `cine serve` daemon.
// Usage: node server/bridge.mjs <project-dir> [--port 7331] [--bin path/to/cine]
import { spawn } from "node:child_process";
import { createInterface } from "node:readline";
import { WebSocketServer } from "ws";

const args = process.argv.slice(2);
const project = args[0];
if (!project) {
  console.error("usage: node server/bridge.mjs <project-dir> [--port 7331] [--bin cine]");
  process.exit(2);
}
const flag = (n, d) => (args.includes(n) ? args[args.indexOf(n) + 1] : d);
const port = Number(flag("--port", "7331"));
const bin = flag("--bin", "../target/debug/cine");

const wss = new WebSocketServer({ port });
console.log(`cine bridge on ws://localhost:${port} → ${bin} serve ${project}`);

wss.on("connection", (ws) => {
  const child = spawn(bin, ["serve", project], { stdio: ["pipe", "pipe", "inherit"] });
  const lines = createInterface({ input: child.stdout });
  lines.on("line", (line) => ws.readyState === ws.OPEN && ws.send(line));
  ws.on("message", (data) => child.stdin.write(data.toString().trim() + "\n"));
  const stop = () => {
    try { child.stdin.write(JSON.stringify({ jsonrpc: "2.0", id: 0, method: "shutdown" }) + "\n"); } catch {}
    setTimeout(() => child.kill(), 500);
  };
  ws.on("close", stop);
  child.on("exit", () => ws.close());
});
