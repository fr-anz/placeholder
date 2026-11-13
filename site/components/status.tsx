export type Tone = "idle" | "plan" | "run" | "rev" | "ok" | "live";

/** A lane or task state, written the way the app writes it. */
export function Status({ tone, children }: { tone: Tone; children: React.ReactNode }) {
  return (
    <span className="status">
      <span className={`dot dot-${tone}`} />
      {children}
    </span>
  );
}
