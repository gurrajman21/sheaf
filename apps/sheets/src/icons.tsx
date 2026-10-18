// Small custom icons where the icon set has nothing that reads clearly.

export type BorderKind = "All" | "Inner" | "Outer" | "Top" | "Right" | "Bottom" | "Left" | "None";

/** A 3x3 cell grid with the edges a border command sets drawn solid. */
export function BorderIcon({ kind, size = 16 }: { kind: BorderKind; size?: number }) {
  const faint = { stroke: "currentColor", strokeOpacity: 0.3, strokeDasharray: "1.5 1.5", strokeWidth: 1 };
  const solid = { stroke: "currentColor", strokeWidth: 1.8 };
  const outer = kind === "All" || kind === "Outer";
  const inner = kind === "All" || kind === "Inner";
  const edge = (on: boolean) => (on ? solid : faint);
  return (
    <svg width={size} height={size} viewBox="0 0 16 16" fill="none" aria-hidden>
      <line x1="2" y1="2" x2="14" y2="2" {...edge(outer || kind === "Top")} />
      <line x1="2" y1="14" x2="14" y2="14" {...edge(outer || kind === "Bottom")} />
      <line x1="2" y1="2" x2="2" y2="14" {...edge(outer || kind === "Left")} />
      <line x1="14" y1="2" x2="14" y2="14" {...edge(outer || kind === "Right")} />
      <line x1="8" y1="2" x2="8" y2="14" {...edge(inner)} />
      <line x1="2" y1="8" x2="14" y2="8" {...edge(inner)} />
    </svg>
  );
}

/** The app mark: a sheet with a highlighted cell. */
export function AppMark({ size = 18 }: { size?: number }) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" aria-hidden>
      <rect x="1" y="1" width="22" height="22" rx="5.5" fill="var(--accent)" />
      <rect x="5" y="5" width="14" height="14" rx="1.6" fill="#fff" />
      <rect x="5" y="5" width="14" height="4.4" fill="#ccf1ea" />
      <path d="M9.7 5v14M14.3 5v14M5 9.7h14M5 14.3h14" stroke="var(--accent)" strokeOpacity=".35" strokeWidth=".8" />
      <rect x="9.7" y="9.7" width="4.6" height="4.6" fill="none" stroke="var(--accent)" strokeWidth="1.4" />
    </svg>
  );
}
