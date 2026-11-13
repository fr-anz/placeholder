"use client";

import { useEffect, useRef } from "react";

const clamp = (value: number) => Math.min(1, Math.max(0, value));

/** Overlapping scroll ranges show parallel workers, with cutover starting last. */
const BRANCHES = [
  { name: "integration", d: "M150 49V386Q150 416 180 416H954", start: 0, end: 1 },
  { name: "cutover", d: "M500 136H850Q890 136 890 176V416", start: 0.5, end: 1 },
  { name: "replay", d: "M150 166Q150 196 180 196H710Q750 196 750 236V416", start: 0.25, end: 0.85 },
  { name: "contracts", d: "M150 226Q150 256 180 256H570Q610 256 610 296V416", start: 0.15, end: 0.7 },
  { name: "signatures", d: "M150 286Q150 316 180 316H430Q470 316 470 356V416", start: 0.05, end: 0.55 },
] as const;

/** Review marks on the lanes: x, y, and whether the review failed. */
const REVIEWS: { x: number; y: number; sentBack?: boolean }[] = [
  { x: 390, y: 316 },
  { x: 530, y: 256 },
  { x: 440, y: 196, sentBack: true },
  { x: 670, y: 196 },
  { x: 810, y: 136 },
];

/** Merges into the integration branch, one at a time, each followed by the check. */
const MERGES = [
  { x: 470 },
  { x: 610 },
  { x: 750 },
  { x: 890 },
];

/** Solid green fills follow the page scroll. There is no timed playback.
    Without JS, or with reduced motion, the diagram is rendered complete. */
export function RunGraph() {
  const frame = useRef<HTMLDivElement>(null);
  const diagram = useRef<SVGSVGElement>(null);
  const keyboardReading = useRef(false);
  const updateProgress = useRef<(() => void) | null>(null);

  useEffect(() => {
    const element = frame.current;
    const graphic = diagram.current;
    if (!element || !graphic) return;

    const paths = [...graphic.querySelectorAll<SVGPathElement>(".g-progress")];
    const reduce = window.matchMedia("(prefers-reduced-motion: reduce)");
    let requestedFrame: number | null = null;
    let previous = -1;

    const update = () => {
      const rect = element.getBoundingClientRect();
      // Spread the fill over 50% more scrolling for a slower progression.
      const distance = Math.max(rect.height, window.innerHeight * 0.6) * 1.5;
      const progress = reduce.matches || keyboardReading.current
        ? 1
        : clamp((window.innerHeight * 0.85 - rect.top) / distance);
      if (progress === previous) return;
      previous = progress;

      paths.forEach((path, index) => {
        const { start, end } = BRANCHES[index];
        const filled = clamp((progress - start) / (end - start));
        path.style.strokeDashoffset = String(1 - filled);
        path.style.opacity = filled === 0 ? "0" : "1";
      });
    };
    // Only schedule frames in response to scrolling or layout changes.
    const schedule = () => {
      if (requestedFrame === null) {
        requestedFrame = window.requestAnimationFrame(() => {
          requestedFrame = null;
          update();
        });
      }
    };
    updateProgress.current = update;
    update();
    window.addEventListener("scroll", schedule, { passive: true });
    window.addEventListener("resize", schedule);
    reduce.addEventListener("change", update);
    const resize = new ResizeObserver(schedule);
    resize.observe(element);

    return () => {
      window.removeEventListener("scroll", schedule);
      window.removeEventListener("resize", schedule);
      reduce.removeEventListener("change", update);
      resize.disconnect();
      if (requestedFrame !== null) window.cancelAnimationFrame(requestedFrame);
      updateProgress.current = null;
      paths.forEach((path) => {
        path.style.removeProperty("stroke-dashoffset");
        path.style.removeProperty("opacity");
      });
    };
  }, []);

  return (
    <>
      <p className="graph-hint label" aria-hidden="true">
        Swipe the diagram to follow the run →
      </p>
      <div
        className="graph-scroll"
        ref={frame}
        tabIndex={0}
        role="group"
        aria-label="Diagram of an orchestrated run"
        onFocus={(event) => {
          if (!event.currentTarget.matches(":focus-visible")) return;
          keyboardReading.current = true;
          updateProgress.current?.();
        }}
        onBlur={() => {
          keyboardReading.current = false;
          updateProgress.current?.();
        }}
      >
        <svg
          ref={diagram}
          className="graph"
          viewBox="0 0 1000 480"
          role="img"
          aria-labelledby="run-graph-title run-graph-desc"
        >
          <title id="run-graph-title">How an orchestrated run moves through Git</title>
          <desc id="run-graph-desc">
            An integration branch is created from main. Four worktrees fork from it, one per assignment. Each is
            reviewed, one is sent back for a fix round, and they merge into the integration branch one at a time with
            a check after every merge. Main is never changed.
          </desc>

          {/* main */}
          <path className="g-main" d="M40 44H960" />
          <circle className="g-ring" cx="150" cy="44" r="5" />
          <text className="g-t g-dim" x="40" y="30">
            main
          </text>
          <text className="g-t g-dim" x="960" y="30" textAnchor="end">
            never merged into
          </text>
          <text className="g-t g-dim" x="166" y="78">
            run starts, uncommitted work included
          </text>

          {/* Queued work stays dotted. Main is never part of the active run. */}
          <path className="g-queued" d="M150 106Q150 136 180 136H500" />
          {BRANCHES.map(({ name, d }) => (
            <path key={name} className={name === "integration" ? "g-int" : "g-lane"} d={d} />
          ))}
          {BRANCHES.map(({ name, d }) => (
            <path key={name} className="g-progress" pathLength={1} d={d} />
          ))}

          <g className="g-t">
            <text x="196" y="124">
              Cutover flag <tspan className="g-dim">OpenCode · queued, waits for Verify signatures</tspan>
            </text>
            <text x="196" y="184">
              Replay protection <tspan className="g-dim">Codex</tspan>
            </text>
            <text x="196" y="244">
              Contract tests <tspan className="g-dim">Cursor</tspan>
            </text>
            <text x="196" y="304">
              Verify signatures <tspan className="g-dim">Claude Code</tspan>
            </text>
          </g>

          {REVIEWS.map(({ x, y, sentBack }) => (
            <g key={`${x}-${y}`}>
              <rect
                className={sentBack ? "g-rev g-bad" : "g-rev"}
                x={x - 5}
                y={y - 5}
                width="10"
                height="10"
                transform={`rotate(45 ${x} ${y})`}
              />
              {sentBack && (
                <text className="g-t g-warn" x={x + 16} y={y + 20}>
                  changes requested, fix round 1 of 2
                </text>
              )}
            </g>
          ))}

          {MERGES.map(({ x }) => (
            <g key={x}>
              <circle className="g-merge" cx={x} cy="416" r="6" />
              <text className="g-t g-ok" x={x} y="442" textAnchor="middle">
                ✓ npm test
              </text>
            </g>
          ))}

          <g>
            <circle className="g-end" cx="960" cy="416" r="7" />
            <text className="g-t" x="960" y="468" textAnchor="end">
              orchestrate/signed-webhooks <tspan className="g-ok">yours to review</tspan>
            </text>
          </g>

        </svg>
      </div>
      <ul className="legend" aria-hidden="true">
        <li>
          <svg viewBox="0 0 14 14">
            <rect className="g-rev" x="3" y="3" width="8" height="8" transform="rotate(45 7 7)" />
          </svg>
          Review passed
        </li>
        <li>
          <svg viewBox="0 0 14 14">
            <rect className="g-rev g-bad" x="3" y="3" width="8" height="8" transform="rotate(45 7 7)" />
          </svg>
          Sent back
        </li>
        <li>
          <svg viewBox="0 0 14 14">
            <circle className="g-merge" cx="7" cy="7" r="5" />
          </svg>
          Merged, check ran
        </li>
      </ul>
    </>
  );
}
