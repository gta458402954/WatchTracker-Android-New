import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'
import { execFileSync } from 'node:child_process'
import { readFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import path from 'node:path'

const projectRoot = fileURLToPath(new URL('.', import.meta.url))
const packageManifest = JSON.parse(readFileSync(path.join(projectRoot, 'package.json'), 'utf8')) as { version?: string }

function resolveGitMetadata() {
  const injectedCommit = process.env.WATCHTRACKER_GIT_COMMIT?.trim()
  const injectedCommitTime = process.env.WATCHTRACKER_GIT_COMMIT_TIME?.trim()
  if (injectedCommit && injectedCommitTime) {
    return { gitCommit: injectedCommit, gitCommitTime: injectedCommitTime }
  }

  try {
    const git = (...args: string[]) => execFileSync('git', args, {
      cwd: projectRoot,
      encoding: 'utf8',
      stdio: ['ignore', 'pipe', 'ignore'],
    }).trim()
    return {
      gitCommit: git('rev-parse', 'HEAD'),
      gitCommitTime: git('show', '-s', '--format=%cI', 'HEAD'),
    }
  } catch {
    return { gitCommit: 'unknown', gitCommitTime: null }
  }
}

const gitMetadata = resolveGitMetadata()
const buildInfo = {
  productVersion: packageManifest.version ?? '0.0.0-dev',
  ...gitMetadata,
}

export default defineConfig({
  plugins: [
    react(),
  ],
  root: projectRoot,
  base: './',
  // 防止 Vite 倾倒不相关的错误
  clearScreen: false,
  server: {
    host: '127.0.0.1',
    port: 5173,
    // Tauri 需要固定的端口
    strictPort: true,
    watch: {
      ignored: ['**/src-tauri/**'],
    },
  },
  // 环境变量前缀
  envPrefix: ['VITE_', 'TAURI_'],
  define: {
    __WATCHTRACKER_BUILD_INFO__: JSON.stringify(buildInfo),
  },
  build: {
    outDir: path.join(projectRoot, 'dist'),
    emptyOutDir: true,
    // 为 Chrome 105+ 或类似的 Tauri 环境进行构建
    target: process.env.TAURI_PLATFORM == 'windows' ? 'chrome105' : 'safari13',
    // 在非 debug 构建中不生成 sourcemap
    sourcemap: !!process.env.TAURI_DEBUG,
  },
  resolve: {
    alias: {
      '@': path.join(projectRoot, 'src'),
    },
  },
})
