// SVG chart renderer for charts in the file and charts added in the app.
// Data is resolved live from the cells by the backend.

export interface ChartSeries { name: string; color: string | null; values: (number | null)[]; cats: string[]; xs: (number | null)[] }
export interface ChartData {
  id: string;
  kind: string;
  title: string | null;
  legend: boolean;
  series: ChartSeries[];
  movable: boolean;
  anchor: { from_col: number; from_row: number; from_dx: number; from_dy: number; to_col: number; to_row: number; to_dx: number; to_dy: number } | null;
  grouping?: string | null;
}

export const CHART_PALETTE = ["#0E7C6B", "#4F5BD5", "#E8A33D", "#D6456A", "#2F9BD6", "#8B5CF6", "#65A30D", "#F97316"];

function niceTicks(min: number, max: number, count = 5): number[] {
  if (min === max) { min -= 1; max += 1; }
  const span = max - min;
  const step0 = span / count;
  const mag = 10 ** Math.floor(Math.log10(step0));
  const step = [1, 2, 2.5, 5, 10].map((m) => m * mag).find((s) => span / s <= count) ?? 10 * mag;
  const lo = Math.floor(min / step) * step;
  const hi = Math.ceil(max / step) * step;
  const out: number[] = [];
  for (let v = lo; v <= hi + step / 2; v += step) out.push(+v.toFixed(10));
  return out;
}

const fmt = (v: number) => {
  const a = Math.abs(v);
  if (a >= 1e9) return `${+(v / 1e9).toFixed(1)}B`;
  if (a >= 1e6) return `${+(v / 1e6).toFixed(1)}M`;
  if (a >= 1e4) return `${+(v / 1e3).toFixed(1)}K`;
  return `${+v.toFixed(2)}`;
};

export function ChartView({ chart, width, height, dark }: { chart: ChartData; width: number; height: number; dark: boolean }) {
  const ink = dark ? "#e6e8eb" : "#1f2328";
  const muted = dark ? "#8f969e" : "#6b7480";
  const grid = dark ? "#33373d" : "#e8ebef";
  const kind = chart.kind;
  const series = chart.series.filter((s) => s.values.length);
  const color = (s: ChartSeries, i: number) => s.color ?? CHART_PALETTE[i % CHART_PALETTE.length];
  const titleH = chart.title ? 30 : 10;
  const pie = kind === "pie" || kind === "doughnut";
  const legendItems = pie
    ? (series[0]?.cats.length ? series[0].cats : series[0]?.values.map((_, i) => `${i + 1}`) ?? []).map((name, i) => ({ name, color: CHART_PALETTE[i % CHART_PALETTE.length] }))
    : series.map((s, i) => ({ name: s.name, color: color(s, i) }));
  const legendH = chart.legend && legendItems.length > 0 ? 26 : 0;
  const pad = { l: 46, r: 16, t: titleH + 6, b: legendH + 26 };
  const w = Math.max(width - pad.l - pad.r, 20);
  const h = Math.max(height - pad.t - pad.b, 20);

  let body: React.ReactNode = null;
  if (!series.length) {
    body = <text x={width / 2} y={height / 2} textAnchor="middle" fill={muted} fontSize={12}>No data</text>;
  } else if (pie) {
    const vals = series[0].values.map((v) => Math.max(v ?? 0, 0));
    const total = vals.reduce((a, b) => a + b, 0) || 1;
    const r = Math.max(Math.min(w + pad.l + pad.r - 24, h + 20) / 2, 10);
    const cx = width / 2, cy = pad.t + (height - pad.t - legendH) / 2 - 4;
    const inner = kind === "doughnut" ? r * 0.56 : 0;
    let a0 = -Math.PI / 2;
    body = vals.map((v, i) => {
      const a1 = a0 + (v / total) * Math.PI * 2;
      const large = a1 - a0 > Math.PI ? 1 : 0;
      const p = (a: number, rr: number) => `${cx + rr * Math.cos(a)},${cy + rr * Math.sin(a)}`;
      const d = inner
        ? `M${p(a0, r)} A${r},${r} 0 ${large} 1 ${p(a1, r)} L${p(a1, inner)} A${inner},${inner} 0 ${large} 0 ${p(a0, inner)} Z`
        : `M${cx},${cy} L${p(a0, r)} A${r},${r} 0 ${large} 1 ${p(a1, r)} Z`;
      const mid = (a0 + a1) / 2;
      const pct = v / total;
      a0 = a1;
      return (
        <g key={i}>
          <path d={d} fill={CHART_PALETTE[i % CHART_PALETTE.length]} stroke={dark ? "#1e2024" : "#fff"} strokeWidth={1.5} />
          {pct > 0.06 && (
            <text x={cx + (inner ? (r + inner) / 2 : r * 0.62) * Math.cos(mid)} y={cy + (inner ? (r + inner) / 2 : r * 0.62) * Math.sin(mid) + 4}
              textAnchor="middle" fontSize={11} fill="#fff" fontWeight={600}>{Math.round(pct * 100)}%</text>
          )}
        </g>
      );
    });
  } else {
    const stacked = chart.grouping === "stacked" || chart.grouping === "percentStacked";
    const n = Math.max(...series.map((s) => s.values.length));
    const cats = series.find((s) => s.cats.length)?.cats ?? Array.from({ length: n }, (_, i) => `${i + 1}`);
    let lo = 0, hi = 0;
    if (stacked) {
      for (let i = 0; i < n; i++) {
        let pos = 0, neg = 0;
        for (const s of series) { const v = s.values[i] ?? 0; if (v >= 0) pos += v; else neg += v; }
        hi = Math.max(hi, pos); lo = Math.min(lo, neg);
      }
    } else {
      for (const s of series) for (const v of s.values) if (v != null) { hi = Math.max(hi, v); lo = Math.min(lo, v); }
    }
    const horizontal = kind === "bar";
    const scatter = kind === "scatter";
    let xlo = 0, xhi = n - 1;
    if (scatter) {
      const xs = series.flatMap((s) => s.xs.filter((x): x is number => x != null));
      if (xs.length) { xlo = Math.min(...xs); xhi = Math.max(...xs); }
    }
    const ticks = niceTicks(lo, hi);
    const t0 = ticks[0], t1 = ticks[ticks.length - 1];
    const valPos = (v: number) => (horizontal ? ((v - t0) / (t1 - t0)) * w : h - ((v - t0) / (t1 - t0)) * h);
    const band = (horizontal ? h : w) / Math.max(n, 1);
    const xticks = scatter ? niceTicks(xlo, xhi) : [];
    const xPos = (x: number) => ((x - (xticks[0] ?? 0)) / ((xticks[xticks.length - 1] ?? 1) - (xticks[0] ?? 0) || 1)) * w;
    const catPos = (i: number) => (i + 0.5) * band;
    const showCat = (i: number) => n <= 14 || i % Math.ceil(n / 14) === 0;

    const axes = (
      <g fontSize={10.5} fill={muted}>
        {ticks.map((t) => (
          horizontal ? (
            <g key={t}>
              <line x1={valPos(t)} x2={valPos(t)} y1={0} y2={h} stroke={grid} />
              <text x={valPos(t)} y={h + 15} textAnchor="middle">{fmt(t)}</text>
            </g>
          ) : (
            <g key={t}>
              <line x1={0} x2={w} y1={valPos(t)} y2={valPos(t)} stroke={grid} />
              <text x={-8} y={valPos(t) + 3.5} textAnchor="end">{fmt(t)}</text>
            </g>
          )
        ))}
        {scatter
          ? xticks.map((t) => <text key={t} x={xPos(t)} y={h + 15} textAnchor="middle">{fmt(t)}</text>)
          : cats.slice(0, n).map((c, i) => showCat(i) && (
            horizontal
              ? <text key={i} x={-8} y={catPos(i) + 3.5} textAnchor="end">{c.length > 10 ? c.slice(0, 9) + "…" : c}</text>
              : <text key={i} x={catPos(i)} y={h + 15} textAnchor="middle">{c.length > 10 ? c.slice(0, 9) + "…" : c}</text>
          ))}
        <line x1={horizontal ? valPos(0) : 0} x2={horizontal ? valPos(0) : w} y1={horizontal ? 0 : valPos(0)} y2={horizontal ? h : valPos(0)} stroke={muted} strokeOpacity={0.5} />
      </g>
    );

    let marks: React.ReactNode[] = [];
    if (kind === "column" || kind === "bar") {
      const groupW = band * 0.72;
      const barW = stacked ? groupW : groupW / series.length;
      const base: number[][] = [Array(n).fill(0), Array(n).fill(0)];
      series.forEach((s, si) => {
        s.values.forEach((v0, i) => {
          const v = v0 ?? 0;
          const from = stacked ? (v >= 0 ? base[0][i] : base[1][i]) : 0;
          const to = from + v;
          if (stacked) { if (v >= 0) base[0][i] = to; else base[1][i] = to; }
          const off = catPos(i) - groupW / 2 + (stacked ? 0 : si * barW);
          const a = valPos(Math.min(from, to)), b = valPos(Math.max(from, to));
          marks.push(horizontal
            ? <rect key={`${si}-${i}`} x={a} y={off} width={Math.max(b - a, 0)} height={barW - 1.5} rx={2} fill={color(s, si)} />
            : <rect key={`${si}-${i}`} x={off} y={b} width={barW - 1.5} height={Math.max(a - b, 0)} rx={2} fill={color(s, si)} />);
        });
      });
    } else {
      marks = series.map((s, si) => {
        const pts = s.values.map((v, i) => (v == null ? null : [scatter && s.xs[i] != null ? xPos(s.xs[i]!) : catPos(i), valPos(v)] as [number, number]))
          .filter((p): p is [number, number] => !!p);
        const d = pts.map((p, i) => `${i ? "L" : "M"}${p[0]},${p[1]}`).join(" ");
        const c = color(s, si);
        return (
          <g key={si}>
            {kind === "area" && pts.length > 1 && <path d={`${d} L${pts[pts.length - 1][0]},${valPos(Math.max(t0, 0))} L${pts[0][0]},${valPos(Math.max(t0, 0))} Z`} fill={c} fillOpacity={0.18} />}
            {!scatter && <path d={d} fill="none" stroke={c} strokeWidth={2.2} strokeLinejoin="round" strokeLinecap="round" />}
            {(scatter || pts.length <= 24) && pts.map((p, i) => <circle key={i} cx={p[0]} cy={p[1]} r={scatter ? 3.5 : 2.6} fill={c} stroke={dark ? "#1e2024" : "#fff"} strokeWidth={1} />)}
          </g>
        );
      });
    }
    body = <g transform={`translate(${pad.l},${pad.t})`}>{axes}{marks}</g>;
  }

  return (
    <svg width={width} height={height} role="img" aria-label={chart.title ?? `${chart.kind} chart`} style={{ display: "block", fontFamily: "-apple-system, BlinkMacSystemFont, system-ui, sans-serif" }}>
      {chart.title && <text x={width / 2} y={21} textAnchor="middle" fontSize={13.5} fontWeight={600} fill={ink}>{chart.title}</text>}
      {body}
      {legendH > 0 && (
        <g transform={`translate(0, ${height - 18})`} fontSize={11} fill={muted}>
          {(() => {
            const items = legendItems.slice(0, 10);
            const widths = items.map((it) => Math.min(it.name.length, 16) * 6.4 + 22);
            const total = widths.reduce((a, b) => a + b, 0);
            let x = Math.max((width - total) / 2, 8);
            return items.map((it, i) => {
              const g = (
                <g key={i} transform={`translate(${x},0)`}>
                  <rect width={9} height={9} y={-8} rx={2.5} fill={it.color} />
                  <text x={14}>{it.name.length > 16 ? it.name.slice(0, 15) + "…" : it.name}</text>
                </g>
              );
              x += widths[i];
              return g;
            });
          })()}
        </g>
      )}
    </svg>
  );
}
