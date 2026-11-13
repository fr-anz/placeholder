import Link from "next/link";
import hero from "@/assets/shots/hero.webp";
import heroNarrow from "@/assets/shots/hero-m.webp";
import { Shot } from "./shot";
import { Mark } from "./mark";
import { DownloadIcon } from "./download-icon";
import { WalkingMinion } from "./walking-minion";

export function Hero() {
  return (
    <section className="hero wrap">
      <h1 className="hero-title">
        <Mark className="mark" animated />
        <span className="hero-wordmark"><span>Placeholder</span></span>
      </h1>
      <div className="hero-intro">
        <p className="lede">
          Run coding agents in parallel. Review their work in one native app.
        </p>
        <Link className="btn" href="/download">
          Download Placeholder
          <DownloadIcon />
        </Link>
      </div>
      <Shot
        className="shot-hero"
        src={hero}
        narrow={heroNarrow}
        priority
        alt="Placeholder showing a Claude Code conversation about rate limiting, with the four changed files listed in a panel on the right."
      >
        <WalkingMinion />
      </Shot>
    </section>
  );
}
