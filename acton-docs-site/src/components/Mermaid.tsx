'use client'

import { useEffect, useId, useRef, useState } from 'react'
import mermaid from 'mermaid'

mermaid.initialize({
  startOnLoad: false,
  theme: 'neutral',
  securityLevel: 'strict',
  fontFamily: 'inherit',
})

export function Mermaid({ chart }: { chart: string }) {
  const id = useId().replace(/:/g, '')
  const containerRef = useRef<HTMLDivElement>(null)
  const [error, setError] = useState<string | null>(null)
  const [rendered, setRendered] = useState(false)

  useEffect(() => {
    const renderChart = async () => {
      if (!chart || !containerRef.current) return

      try {
        // Clear any previous content
        while (containerRef.current.firstChild) {
          containerRef.current.removeChild(containerRef.current.firstChild)
        }

        // Create a temporary container for mermaid to render into
        const tempDiv = document.createElement('div')
        tempDiv.id = `mermaid-${id}`
        tempDiv.textContent = chart.trim()
        containerRef.current.appendChild(tempDiv)

        // Let mermaid render the diagram in place
        await mermaid.run({
          nodes: [tempDiv],
        })

        setError(null)
        setRendered(true)
      } catch (err) {
        setError(err instanceof Error ? err.message : 'Failed to render diagram')
        console.error('Mermaid rendering error:', err)
      }
    }

    renderChart()
  }, [chart, id])

  if (error) {
    return (
      <div className="my-6 rounded-lg border border-red-200 bg-red-50 p-4 dark:border-red-900 dark:bg-red-950">
        <p className="text-sm text-red-600 dark:text-red-400">
          Failed to render Mermaid diagram: {error}
        </p>
        <pre className="mt-2 overflow-auto text-xs text-red-500 dark:text-red-400">
          {chart}
        </pre>
      </div>
    )
  }

  return (
    <div
      ref={containerRef}
      className="my-6 flex justify-center overflow-auto rounded-lg bg-slate-50 p-4 dark:bg-slate-800/50"
      aria-label="Mermaid diagram"
    />
  )
}
