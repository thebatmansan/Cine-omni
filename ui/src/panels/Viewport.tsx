import { useEffect, useRef } from "react";
import { RpcClient } from "../rpc/client";

interface Props {
  client: RpcClient;
  scene: string | null;
  revision: number;
}

interface Placed {
  name: string;
  kind: "character" | "camera" | "light";
  x: number;
  z: number;
  focal?: number;
}

/**
 * Top-down blocking view of the derived previs scene: characters, cameras with
 * their horizontal field of view, and lights. Uses Canvas 2D so it runs
 * everywhere; the WebGPU viewport (cine-gpu `previs` kernel) replaces this
 * when the WASM build lands.
 */
export function Viewport({ client, scene, revision }: Props) {
  const ref = useRef<HTMLCanvasElement>(null);

  useEffect(() => {
    if (!scene) return;
    let cancelled = false;
    (async () => {
      const previsId = await derivedPrevis(client, scene);
      if (!previsId) return;
      const previs = await client.node(previsId);
      const kids = await Promise.all(previs.children.map((c) => client.node(c)));
      const placed: Placed[] = [];
      for (const k of kids) {
        const t = k.attrs.transform as any;
        const tr = t?.translate ?? t?.v?.translate ?? [0, 0, 0];
        const kind = k.kind.endsWith("character") ? "character" : k.kind.endsWith("camera") ? "camera" : k.kind.endsWith("light") ? "light" : null;
        if (!kind) continue;
        const cam = k.attrs.camera as any;
        placed.push({ name: String(k.attrs.name ?? k.id.slice(0, 8)), kind, x: tr[0], z: tr[2], focal: cam?.focal_length_mm ?? cam?.v?.focal_length_mm });
      }
      if (!cancelled) draw(ref.current, placed);
    })();
    return () => { cancelled = true; };
  }, [client, scene, revision]);

  return <canvas ref={ref} className="viewport" width={800} height={500} />;
}

async function derivedPrevis(client: RpcClient, scene: string): Promise<string | null> {
  // The previs scene is the one whose DerivedFrom edge targets the script scene.
  const all = await client.ofKind("previs.scene");
  for (const p of all) {
    const n = await client.node(p.id);
    if (n.edges_out.some((e) => e.kind === "derived_from" && e.to === scene)) return p.id;
  }
  return null;
}

function draw(canvas: HTMLCanvasElement | null, placed: Placed[]) {
  if (!canvas) return;
  const ctx = canvas.getContext("2d");
  if (!ctx) return;
  const { width, height } = canvas;
  ctx.clearRect(0, 0, width, height);
  const scale = 40; // px per metre
  const cx = width / 2, cz = height / 2;
  ctx.strokeStyle = "#20232a";
  for (let m = -10; m <= 10; m++) {
    ctx.beginPath(); ctx.moveTo(cx + m * scale, 0); ctx.lineTo(cx + m * scale, height); ctx.stroke();
    ctx.beginPath(); ctx.moveTo(0, cz + m * scale); ctx.lineTo(width, cz + m * scale); ctx.stroke();
  }
  for (const p of placed) {
    const x = cx + p.x * scale, y = cz + p.z * scale;
    if (p.kind === "camera") {
      // Horizontal FOV for a 27.99 mm wide sensor (ALEXA 35 default).
      const fov = 2 * Math.atan(27.99 / (2 * (p.focal ?? 35)));
      ctx.fillStyle = "rgba(242,169,59,0.12)";
      ctx.beginPath();
      ctx.moveTo(x, y);
      ctx.lineTo(x + Math.sin(-fov / 2) * 300, y - Math.cos(-fov / 2) * 300);
      ctx.lineTo(x + Math.sin(fov / 2) * 300, y - Math.cos(fov / 2) * 300);
      ctx.closePath();
      ctx.fill();
      ctx.fillStyle = "#f2a93b";
      ctx.fillRect(x - 6, y - 6, 12, 12);
    } else if (p.kind === "character") {
      ctx.fillStyle = "#9fd3ff";
      ctx.beginPath(); ctx.arc(x, y, 8, 0, Math.PI * 2); ctx.fill();
    } else {
      ctx.fillStyle = "#fff3a0";
      ctx.beginPath(); ctx.arc(x, y, 5, 0, Math.PI * 2); ctx.fill();
    }
    ctx.fillStyle = "#e6e6e6";
    ctx.font = "11px sans-serif";
    ctx.fillText(p.name, x + 10, y + 4);
  }
}
