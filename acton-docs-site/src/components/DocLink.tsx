import Link from 'next/link'

export function DocLink({
  href,
  children,
  ...props
}: React.ComponentPropsWithoutRef<'a'>) {
  // Internal links must go through next/link so the production basePath
  // (/acton-reactive on GitHub Pages) is applied; a raw <a> escapes it and 404s.
  if (href?.startsWith('/')) {
    return (
      <Link href={href} {...props}>
        {children}
      </Link>
    )
  }
  return (
    <a href={href} {...props}>
      {children}
    </a>
  )
}
