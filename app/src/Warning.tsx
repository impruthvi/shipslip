/** The void triangle used by banners and cell warnings. */
export function WarningIcon() {
  return (
    <svg className="warning-icon" width="12" height="12" viewBox="0 0 12 12" aria-hidden="true">
      <path d="M6 1 L11 10.5 H1 Z" fill="none" strokeWidth="1.3" strokeLinejoin="round" />
      <path d="M6 4.8 V7.2" fill="none" strokeWidth="1.3" strokeLinecap="round" />
    </svg>
  );
}
