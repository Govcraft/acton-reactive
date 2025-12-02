import { VERSION } from '@/lib/version'

function LogomarkPaths() {
  return (
    <g>
      {/* Connection lines between actors */}
      <path
        d="M8 18 L16 10 M16 10 L28 14 M16 10 L18 26 M28 14 L18 26"
        fill="none"
        stroke="url(#logo-gradient)"
        strokeWidth={2}
        strokeLinecap="round"
      />
      {/* Actor nodes */}
      <circle cx="8" cy="18" r="4" fill="#F59E0B" />
      <circle cx="16" cy="10" r="5" fill="#F97316" />
      <circle cx="28" cy="14" r="4" fill="#FBBF24" />
      <circle cx="18" cy="26" r="4" fill="#F59E0B" />
      {/* Gradient definition */}
      <defs>
        <linearGradient id="logo-gradient" x1="0%" y1="0%" x2="100%" y2="100%">
          <stop offset="0%" stopColor="#FBBF24" />
          <stop offset="100%" stopColor="#F97316" />
        </linearGradient>
      </defs>
    </g>
  )
}

export function Logomark(props: React.ComponentPropsWithoutRef<'svg'>) {
  return (
    <svg aria-hidden="true" viewBox="0 0 36 36" fill="none" {...props}>
      <LogomarkPaths />
    </svg>
  )
}

export function Logo(props: React.ComponentPropsWithoutRef<'svg'>) {
  return (
    <svg aria-hidden="true" viewBox="0 0 260 36" fill="none" {...props}>
      <LogomarkPaths />
      <text
        x="40"
        y="24"
        fill="currentColor"
        fontFamily="ui-sans-serif, system-ui, sans-serif"
        fontSize="18"
        fontWeight="600"
        letterSpacing="-0.02em"
      >
        acton-reactive
      </text>
      <text
        x="180"
        y="24"
        fill="currentColor"
        fontFamily="ui-sans-serif, system-ui, sans-serif"
        fontSize="14"
        fontWeight="400"
        opacity="0.6"
      >
        v{VERSION}
      </text>
    </svg>
  )
}
