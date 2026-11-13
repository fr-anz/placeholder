"use client";

import Image from "next/image";
import { useEffect, useRef, useState } from "react";
import { AGENTS } from "@/lib/site";

function AgentLogos({ duplicate = false }: { duplicate?: boolean }) {
  return (
    <ul className="agents-group" aria-hidden={duplicate || undefined}>
      {AGENTS.map(({ name, logo }) => (
        <li className="agent-logo" key={name}>
          <Image src={`/agents/${logo}.svg`} width={40} height={40} alt="" loading="eager" unoptimized />
          <span>{name}</span>
        </li>
      ))}
    </ul>
  );
}

export function Agents() {
  const frame = useRef<HTMLElement>(null);
  const [visibility, setVisibility] = useState<"pending" | "visible" | "hidden">("pending");
  const [paused, setPaused] = useState(false);

  useEffect(() => {
    if (!frame.current || !("IntersectionObserver" in window)) return;
    const observer = new IntersectionObserver(([entry]) => {
      setVisibility(entry.isIntersecting ? "visible" : "hidden");
    });
    observer.observe(frame.current);
    return () => observer.disconnect();
  }, []);

  return (
    <section
      ref={frame}
      className="agents wrap"
      aria-label="Supported agents"
      data-ready={visibility !== "pending"}
      data-playing={visibility === "visible" && !paused}
    >
      <div
        className="agents-window"
        tabIndex={0}
        role="group"
        aria-label="Agent logos. Focus to pause; tap to pause or resume."
        onPointerDown={(event) => {
          if (event.pointerType !== "mouse") setPaused((value) => !value);
        }}
      >
        <div className="agents-track">
          <AgentLogos />
          <AgentLogos duplicate />
        </div>
      </div>
    </section>
  );
}
