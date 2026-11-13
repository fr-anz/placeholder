/** The app icon: a ring of squares fading out around a prompt. */
const RING: [number, number, number][] = [
  [5, 12, 1],
  [12, 5, 0.82],
  [21, 5, 0.7],
  [28, 12, 0.58],
  [28, 21, 0.46],
  [21, 28, 0.36],
  [12, 28, 0.26],
  [5, 21, 0.16],
];

export function Mark({ className, animated = false }: { className?: string; animated?: boolean }) {
  return (
    <svg className={className} viewBox="0 0 40 40" aria-hidden="true">
      {RING.map(([x, y, opacity], index) => (
        <g key={`${x}-${y}`} className={animated ? "mark-tile" : undefined} style={animated ? { animationDelay: `${index * 40}ms` } : undefined}>
          <rect x={x} y={y} width="7" height="7" opacity={opacity} />
        </g>
      ))}
      <rect x="16.5" y="16.5" width="7" height="7" />
    </svg>
  );
}
