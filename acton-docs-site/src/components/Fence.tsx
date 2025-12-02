'use client'

import { Fragment } from 'react'
import { Highlight } from 'prism-react-renderer'
import { Mermaid } from './Mermaid'

export function Fence({
  children,
  language,
}: {
  children: string
  language?: string
}) {
  if (language === 'mermaid') {
    return <Mermaid chart={children} />
  }

  // Default to 'text' when no language is specified to prevent
  // prism-react-renderer from crashing on undefined.toLowerCase()
  const lang = language || 'text'

  return (
    <Highlight
      code={children.trimEnd()}
      language={lang}
      theme={{ plain: {}, styles: [] }}
    >
      {({ className, style, tokens, getTokenProps }) => (
        <pre className={className} style={style}>
          <code>
            {tokens.map((line, lineIndex) => (
              <Fragment key={lineIndex}>
                {line
                  .filter((token) => !token.empty)
                  .map((token, tokenIndex) => (
                    <span key={tokenIndex} {...getTokenProps({ token })} />
                  ))}
                {'\n'}
              </Fragment>
            ))}
          </code>
        </pre>
      )}
    </Highlight>
  )
}
