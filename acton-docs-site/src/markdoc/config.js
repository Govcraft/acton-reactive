import nodes from './nodes.js'
import tags from './tags.js'
import { siteConfig } from '../lib/config'

// Extract version from workspace Cargo.toml
// This should be kept in sync with the workspace version
const ACTON_VERSION = '7.0.0'

// Helper function to build dependency string
function buildDep(features) {
  return `acton-reactive = { version = "${ACTON_VERSION}", features = [${features.map(f => `"${f}"`).join(', ')}] }`
}

const config = {
  nodes,
  tags,
  functions: {
    // Markdoc function to build cargo dependency with current version
    dep: {
      transform(parameters) {
        const features = parameters[0] || []
        if (typeof features === 'string') {
          return `acton-reactive = { version = "${ACTON_VERSION}", features = ["${features}"] }`
        }
        return buildDep(features)
      }
    },
    // Function to build GitHub URLs
    githubUrl: {
      transform(parameters) {
        const path = parameters[0] || ''
        return siteConfig.repositoryUrl + path
      }
    }
  },
  variables: {
    version: {
      acton: ACTON_VERSION,
    },
    github: {
      repositoryUrl: siteConfig.repositoryUrl,
      repositoryName: siteConfig.repositoryName,
    },
    dep: {
      base: `acton-reactive = "${ACTON_VERSION}"`,
      ipc: `acton-reactive = { version = "${ACTON_VERSION}", features = ["ipc"] }`,
      ipcMessagepack: `acton-reactive = { version = "${ACTON_VERSION}", features = ["ipc-messagepack"] }`,
    },
  },
}

export default config
