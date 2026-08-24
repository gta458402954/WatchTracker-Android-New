import React from 'react'
import ReactDOM from 'react-dom/client'
import App from './app/App.tsx'
import ErrorBoundary from './shared/components/ErrorBoundary.tsx'
import './index.css'
import { isAndroidRuntime } from './platform/runtime'
import MobileApp from './app/MobileApp'

// 立即挂载 React 应用
ReactDOM.createRoot(document.getElementById('root')!).render(
  <React.StrictMode>
    <ErrorBoundary>
      {isAndroidRuntime() ? <MobileApp /> : <App />}
    </ErrorBoundary>
  </React.StrictMode>,
)
