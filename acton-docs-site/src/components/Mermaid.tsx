'use client'

import { useEffect, useRef, useState } from 'react'
import mermaid from 'mermaid'

mermaid.initialize({
  startOnLoad: false,
  theme: 'neutral',
  securityLevel: 'strict',
  fontFamily: 'inherit',
})

let counter = 0

export function Mermaid({ chart }: { chart: string }) {
  const containerRef = useRef<HTMLDivElement>(null)
  const [error, setError] = useState<string | null>(null)
  const idRef = useRef(`mermaid-${++counter}`)

  useEffect(() => {
    const render = async () => {
      if (!chart || !containerRef.current) return

      try {
        // Clear previous
        while (containerRef.current.firstChild) {
          containerRef.current.removeChild(containerRef.current.firstChild)
        }

        // Create div for mermaid with unique ID each render
        const div = document.createElement('div')
        div.id = `${idRef.current}-${Date.now()}`
        div.textContent = chart.trim()
        containerRef.current.appendChild(div)

        // Render
        await mermaid.run({ nodes: [div] })
        setError(null)
      } catch (err) {
        setError(err instanceof Error ? err.message : 'Render failed')
        console.error('Mermaid error:', err)
      }
    }

    render()
  }, [chart])

  if (error) {
    return (
      <div className="my-6 rounded-lg border border-red-200 bg-red-50 p-4 dark:border-red-900 dark:bg-red-950">
        <p className="text-sm text-red-600 dark:text-red-400">
          Failed to render diagram: {error}
        </p>
        <pre className="mt-2 overflow-auto text-xs text-red-500">{chart}</pre>
      </div>
    )
  }

  return (
    <div
      ref={containerRef}
      className="my-6 w-full overflow-x-auto rounded-lg bg-zinc-50 p-4 dark:bg-zinc-800/50 [&_svg]:mx-auto [&_svg]:block"
      aria-label="Mermaid diagram"
    />
  )
}
