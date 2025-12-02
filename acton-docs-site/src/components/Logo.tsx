import { VERSION } from '@/lib/version'

function LogomarkPaths() {
  return (
    <g fill="none" stroke="#38BDF8" strokeLinejoin="round" strokeWidth={2}>
      {/* Stylized "A" representing an Actor */}
      <path d="M18 6 L6 30 M18 6 L30 30 M10 22 L26 22" />
      {/* Message arrow flowing through */}
      <path d="M14 14 L22 14 L20 11 M22 14 L20 17" strokeWidth={1.5} />
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
