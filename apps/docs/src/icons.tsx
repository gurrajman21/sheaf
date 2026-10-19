// The app mark: a folded page with lines of text.

export function AppMark({ size = 18 }: { size?: number }) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" aria-hidden>
      <rect x="1" y="1" width="22" height="22" rx="5.5" fill="var(--accent)" />
      <path d="M6.5 4.8h7.8l3.4 3.4v11a.9.9 0 0 1-.9.9H6.5a.9.9 0 0 1-.9-.9V5.7a.9.9 0 0 1 .9-.9Z" fill="#fff" />
      <path d="M14.3 4.8v2.6a.8.8 0 0 0 .8.8h2.6" fill="#dbe4fb" />
      <path d="M8.2 10.2h7.6M8.2 12.8h7.6M8.2 15.4h5" stroke="var(--accent)" strokeWidth="1.15" strokeLinecap="round" />
    </svg>
  );
}
