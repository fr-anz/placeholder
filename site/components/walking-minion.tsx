"use client";

import { useEffect, useRef, useState } from "react";
import dino from "@/assets/minion/dino.json";

/** The desktop app's actual 8 × 8 dinosaur, rendered without a bitmap atlas. */
function Pixels({ rows, from = 0, to = rows.length }: { rows: string[]; from?: number; to?: number }) {
  return rows.flatMap((row, y) => y < from || y >= to ? [] : [...row].flatMap((pixel, x) => (
    pixel === "#" ? [
      <rect
        key={`${x}-${y}`}
        x={x}
        y={y}
        width="1"
        height="1"
        fill={y === 2 && x >= 5 ? "#ece0ae" : y < 3 ? "#c58cdd" : y > 4 ? "#a562c1" : "#b477cf"}
      />,
    ] : []
  )));
}

export function WalkingMinion() {
  const frame = useRef<HTMLDivElement>(null);
  const [walking, setWalking] = useState(false);

  useEffect(() => {
    if (!frame.current || !("IntersectionObserver" in window)) return;
    const observer = new IntersectionObserver(([entry]) => setWalking(entry.isIntersecting));
    observer.observe(frame.current);
    return () => observer.disconnect();
  }, []);

  return (
    <div ref={frame} className="hero-minion" data-walking={walking} aria-hidden="true">
      <div className="minion-route">
        <div className="minion-facing">
          <svg className="minion-body" viewBox="0 0 8 8" width="24" height="24" shapeRendering="crispEdges" focusable="false">
            <Pixels rows={dino.rest} to={5} />
            <g className="minion-step-a"><Pixels rows={dino.rest} from={5} /></g>
            <g className="minion-step-b"><Pixels rows={dino.talk} from={5} /></g>
            <rect x="5" y="1" width="1" height="1" fill="#263331" />
            <rect x="5.2" y="1.2" width="0.3" height="0.3" fill="#fffff4" />
          </svg>
        </div>
      </div>
    </div>
  );
}
